import base64
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import export_image

# Small local fixture, never fetched from a real provider.
PNG = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jCzYAAAAASUVORK5CYII=')
JPEG = b'\xff\xd8\xff\xe0local-test-fixture\xff\xd9'


def text_result(value):
    return {'isError': False, 'content': [{'type': 'text', 'text': json.dumps(value)}]}


def bundle(provider='flux', cost=None, execution='exec-1', artifact='artifact-1', jpeg=False):
    """Current public unified-MCP schemas, including safe identities and exact costs."""
    cost = cost or {'amount': '6', 'scale': 2, 'currency': 'USD'}
    identity = {'id': next(k for k, v in export_image.PROVIDERS.items() if v == provider),
                'display_name': provider}
    data = JPEG if jpeg else PNG
    metadata = {'schema_version': 1, 'artifact_id': artifact,
                'media_type': 'image/jpeg' if jpeg else 'image/png',
                'size_bytes': len(data), 'sha256': 'sha256:' + hashlib.sha256(data).hexdigest(),
                'encoding': 'base64'}
    row = {'id': 'ledger-1', 'authorization_id': 'decision-1', 'settlement_id': 'settlement-1',
           'provider': identity, 'effective_cost': cost, 'cost_semantics': 'original_total'}
    record = {'authorization_id': 'decision-1', 'status': 'settled', 'provider': identity,
              'agent_id': 'agt_1', 'account_id': 'aga_1', 'ledger_transaction_ids': ['ledger-1'],
              'receipt': {'settlement_id': 'settlement-1', 'actual_vendor_cost': cost,
                          'budget_charge_cents': 6}}
    result = text_result(metadata)
    result['content'].append({'type': 'image', 'mimeType': metadata['media_type'],
                              'data': base64.b64encode(data).decode()})
    return {'operation': {'isError': False, 'content': [], 'structuredContent': {
                'schema_version': 1, 'state': 'succeeded', 'terminal': True,
                'operation_handle': 'op-1', 'execution_id': execution,
                'authorization': {'decision_id': 'decision-1'}}},
            'authorization': text_result({'schema_version': 'hubu-history-v1', 'authorization_record': record}),
            'authorization_after_ledger': text_result({'schema_version': 'hubu-history-v1', 'authorization_record': record}),
            'ledger': text_result({'schema_version': 'hubu-history-v1', 'transactions': [row],
                                   'coverage': None, 'next_cursor': None}),
            'artifact_list': text_result({'schema_version': 1, 'execution_id': execution,
                                          'artifacts': [dict(metadata, execution_id=execution, kind='image')]}),
            'artifact': result, 'size': '2k', 'tier': 'draft'}


def edit_result(inputs, key, edit):
    value = export_image.public_json(inputs[key])
    edit(value)
    inputs[key] = text_result(value)


class GalleryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.output = Path(self.directory.name) / 'gallery'

    def test_both_providers_sequential_and_jpeg_bytes(self):
        a = export_image.export(bundle(), self.output)
        b = export_image.export(bundle('gemini', execution='exec-2', jpeg=True), self.output)
        self.assertEqual(a.name, '01-flux-2k-draft-6c.png')
        self.assertEqual(b.name, '02-gemini-2k-draft-6c.jpg')
        self.assertEqual(a.read_bytes(), PNG)
        self.assertEqual(b.read_bytes(), JPEG)
        receipt = json.loads(b.with_suffix('.jpg.receipt.json').read_text())
        self.assertEqual(receipt['settled_cost'], {'amount': '6', 'scale': 2, 'currency': 'USD'})
        self.assertEqual(receipt['cost_semantics'], 'operation_total_not_per_image')

    def test_exact_fractional_cent_is_not_rounded_budget_charge(self):
        inputs = bundle('gemini', {'amount': '1', 'scale': 3, 'currency': 'USD'})
        edit_result(inputs, 'authorization', lambda v: v['authorization_record']['receipt'].update(budget_charge_cents=1))
        inputs['authorization_after_ledger'] = copy.deepcopy(inputs['authorization'])
        path = export_image.export(inputs, self.output)
        self.assertEqual(path.name, '01-gemini-2k-draft-0.1c.png')
        self.assertEqual(export_image.exact_cost({'amount': '123456789012345678901234567890', 'scale': 18, 'currency': 'USD'}),
                         '12345678901234.567890123456789')

    def test_resumed_status_and_json_rpc_replay_are_idempotent(self):
        inputs = bundle()
        original = copy.deepcopy(inputs['operation'])
        del inputs['operation']['structuredContent']['authorization']
        inputs['submission'] = original
        first = export_image.export(inputs, self.output)
        inputs['operation'] = {'jsonrpc': '2.0', 'id': 3, 'result': inputs['operation']}
        self.assertEqual(export_image.export(inputs, self.output), first)
        self.assertEqual(len(list(self.output.glob('*.png'))), 1)
        self.assertEqual(len(json.loads((self.output / '.gallery.json').read_text())), 1)

    def test_optional_mcp_error_flag_and_updated_resume_identity(self):
        inputs = bundle()
        value = inputs['operation']['structuredContent']
        inputs['submission'] = copy.deepcopy(inputs['operation'])
        inputs['submission']['structuredContent']['hubu_result'] = value['authorization']
        del inputs['submission']['structuredContent']['authorization']
        del value['authorization']
        for result in inputs.values():
            if isinstance(result, dict):
                result.pop('isError', None)
        self.assertTrue(export_image.export(inputs, self.output).exists())

    def test_resume_hubu_result_projection(self):
        inputs = bundle()
        value = inputs['operation']['structuredContent']
        value['hubu_result'] = value.pop('authorization')
        self.assertTrue(export_image.export(inputs, self.output).exists())

    def test_every_image_in_same_operation_has_total_cost(self):
        first = export_image.export(bundle(artifact='artifact-1'), self.output)
        second = export_image.export(bundle(artifact='artifact-2'), self.output)
        self.assertEqual([first.name, second.name], ['01-flux-2k-draft-6c.png', '02-flux-2k-draft-6c.png'])
        manifest = json.loads((self.output / '.gallery.json').read_text())
        self.assertEqual([entry['execution_id'] for entry in manifest], ['exec-1', 'exec-1'])
        self.assertTrue(all(entry['cost_semantics'] == 'operation_total_not_per_image' for entry in manifest))

    def test_status_cannot_use_other_submission_handle(self):
        inputs = bundle()
        inputs['submission'] = copy.deepcopy(inputs['operation'])
        del inputs['operation']['structuredContent']['authorization']
        inputs['submission']['structuredContent']['operation_handle'] = 'other'
        with self.assertRaisesRegex(ValueError, 'handle mismatch'):
            export_image.export(inputs, self.output)

    def test_pending_operation_never_writes(self):
        inputs = bundle()
        inputs['operation']['structuredContent']['state'] = 'approval_required'
        with self.assertRaisesRegex(ValueError, 'has not succeeded'):
            export_image.export(inputs, self.output)
        self.assertFalse(self.output.exists())

    def test_mismatched_settlement_and_missing_ledger_refuse(self):
        for edit, message in [(lambda v: v['transactions'][0].update(settlement_id='other'), 'settlement'),
                              (lambda v: v.update(transactions=[]), 'remaining ledger pages'),
                              (lambda v: v['transactions'][0].update(effective_cost={'amount': '7', 'scale': 2, 'currency': 'USD'}), 'cost mismatch'),
                              (lambda v: v['transactions'][0].update(cost_semantics='corrected_total'), 'reconciliation')]:
            inputs = bundle()
            edit_result(inputs, 'ledger', edit)
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                export_image.export(inputs, self.output)
        self.assertFalse(self.output.exists())

    def test_unlisted_correction_racing_initial_record_refuses(self):
        inputs = bundle()
        def add_correction(history):
            correction = copy.deepcopy(history['transactions'][0])
            correction.update(id='correction-1', cost_semantics='corrected_total',
                              effective_cost={'amount': '8', 'scale': 2, 'currency': 'USD'})
            history['transactions'].insert(0, correction)
        edit_result(inputs, 'ledger', add_correction)
        with self.assertRaisesRegex(ValueError, 'unlisted settlement posting'):
            export_image.export(inputs, self.output)
        self.assertFalse(self.output.exists())

    def test_correction_outside_pagination_upper_bound_refuses(self):
        inputs = bundle()
        edit_result(inputs, 'authorization_after_ledger', lambda value:
                    value['authorization_record']['ledger_transaction_ids'].append('new-correction'))
        with self.assertRaisesRegex(ValueError, 'changed during pagination'):
            export_image.export(inputs, self.output)
        self.assertFalse(self.output.exists())

    def test_missing_final_accounting_read_refuses(self):
        inputs = bundle()
        del inputs['authorization_after_ledger']
        with self.assertRaises(KeyError):
            export_image.export(inputs, self.output)
        self.assertFalse(self.output.exists())

    def test_foreign_artifact_and_corrupt_bytes_refuse(self):
        inputs = bundle()
        edit_result(inputs, 'artifact_list', lambda v: v['artifacts'][0].update(execution_id='other'))
        with self.assertRaisesRegex(ValueError, 'bound'):
            export_image.export(inputs, self.output)
        inputs = bundle()
        inputs['artifact']['content'][1]['data'] = base64.b64encode(PNG[:-1] + b'x').decode()
        with self.assertRaisesRegex(ValueError, 'digest'):
            export_image.export(inputs, self.output)

    def test_interrupted_write_repairs_reserved_sequence(self):
        real_write = export_image.atomic_write
        def interrupted(path, data):
            if path.suffix == '.png':
                raise OSError('simulated interruption')
            real_write(path, data)
        with patch.object(export_image, 'atomic_write', interrupted), self.assertRaises(OSError):
            export_image.export(bundle(), self.output)
        self.assertEqual(export_image.export(bundle(), self.output).name, '01-flux-2k-draft-6c.png')

    def test_changed_existing_image_not_overwritten(self):
        path = export_image.export(bundle(), self.output)
        path.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'image changed'):
            export_image.export(bundle(), self.output)
        self.assertEqual(path.read_bytes(), b'changed')

    def test_symlink_output_refused(self):
        self.output.symlink_to(Path(self.directory.name), target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'non-symlink'):
            export_image.export(bundle(), self.output)

    def test_concurrent_cli_exports_share_sequence(self):
        script = Path(export_image.__file__)
        processes = []
        for index in range(4):
            file = Path(self.directory.name) / f'input-{index}.json'
            file.write_text(json.dumps(bundle('gemini' if index % 2 else 'flux', execution=f'exec-{index}')))
            processes.append(subprocess.Popen([sys.executable, str(script), '--input', str(file), '--output', str(self.output)],
                                              stdout=subprocess.PIPE, stderr=subprocess.PIPE))
        for process in processes:
            _, error = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0, error)
        manifest = json.loads((self.output / '.gallery.json').read_text())
        self.assertEqual(sorted(entry['sequence'] for entry in manifest), [1, 2, 3, 4])
        self.assertEqual(len(list(self.output.glob('*.png'))), 4)


if __name__ == '__main__':
    unittest.main()
