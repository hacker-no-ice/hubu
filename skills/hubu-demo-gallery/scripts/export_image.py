#!/usr/bin/env python3
"""Export existing unified-MCP images locally; no network or provider calls."""
import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

PROVIDERS = {'provider:black-forest-labs:flux': 'flux',
             'provider:google:gemini-developer': 'gemini'}
MAX_BYTES = 64 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def public_json(result):
    result = result.get('result', result)
    require(result.get('isError', False) is False, 'MCP result is not successful')
    if 'structuredContent' in result:
        return result['structuredContent']
    texts = [c['text'] for c in result['content'] if c.get('type') == 'text']
    require(len(texts) == 1, 'expected one JSON text block')
    return json.loads(texts[0])


def exact_cost(cost):
    require(cost.get('currency') == 'USD', 'demo filenames require USD')
    amount, scale = cost.get('amount'), cost.get('scale')
    require(isinstance(amount, str) and re.fullmatch(r'[0-9]{1,39}', amount),
            'cost must be a nonnegative coefficient string')
    require(type(scale) is int and 0 <= scale <= 18, 'invalid cost scale')
    # Integer-only rendering preserves sub-cent costs without rounding.
    decimals = max(scale - 2, 0)
    coefficient = int(amount) * 10 ** max(2 - scale, 0)
    digits = str(coefficient).zfill(decimals + 1)
    label = digits[:-decimals] + '.' + digits[-decimals:] if decimals else digits
    return label.rstrip('0').rstrip('.') if '.' in label else label


def validated_export(bundle):
    operation = public_json(bundle['operation'])
    require(operation.get('state') == 'succeeded' and operation.get('terminal') is True,
            'operation has not succeeded; observe/resume it, never rerun provider')
    execution_id = operation['execution_id']
    identity = operation.get('authorization') or operation.get('hubu_result')
    if identity is None:
        submission = public_json(bundle['submission'])
        require(submission.get('operation_handle') == operation.get('operation_handle')
                and isinstance(operation.get('operation_handle'), str),
                'submission/status operation handle mismatch')
        identity = submission.get('authorization') or submission['hubu_result']
    authorization_id = identity['decision_id']
    authorization = public_json(bundle['authorization'])
    require(authorization.get('schema_version') == 'hubu-history-v1', 'unknown history schema')
    record = authorization['authorization_record']
    require(record.get('authorization_id') == authorization_id and record.get('status') == 'settled',
            'authorization is mismatched or not settled')
    receipt = record['receipt']
    settlement_id = receipt['settlement_id']
    require(isinstance(settlement_id, str) and settlement_id, 'missing settlement identity')
    history = public_json(bundle['ledger'])
    require(history.get('schema_version') == 'hubu-history-v1', 'unknown ledger schema')
    rows, linked_ids = history['transactions'], record['ledger_transaction_ids']
    require(linked_ids and len(set(linked_ids)) == len(linked_ids), 'missing/duplicate ledger links')
    linked = [row for row in rows if row['id'] in linked_ids]
    require(len(linked) == len(linked_ids), 'fetch remaining ledger pages before export')
    require(all(row.get('authorization_id') == authorization_id and
                row.get('settlement_id') == settlement_id for row in linked),
            'ledger settlement does not match authorization receipt')
    require(len(linked) == 1 and linked[0].get('cost_semantics') == 'original_total',
            'corrected/multiple ledger postings require gallery reconciliation')
    row = linked[0]
    require(row['effective_cost'] == receipt['actual_vendor_cost'], 'ledger/receipt cost mismatch')
    provider_id = record['provider']['id']
    require(provider_id in PROVIDERS and row['provider']['id'] == provider_id,
            'unsupported or mismatched image provider')
    cost = row['effective_cost']
    cost_label = exact_cost(cost)
    size, tier = bundle['size'], bundle['tier']
    require(size in ('512', '1k', '2k', '4k', 'custom'), 'unsupported size label')
    require(tier in ('draft', 'final'), 'unsupported demo tier')
    listing = public_json(bundle['artifact_list'])
    require(listing['execution_id'] == execution_id, 'artifact list execution mismatch')
    artifact_result = bundle['artifact'].get('result', bundle['artifact'])
    metadata = public_json(artifact_result)
    matches = [item for item in listing['artifacts'] if item['artifact_id'] == metadata['artifact_id']]
    require(len(matches) == 1 and matches[0]['execution_id'] == execution_id,
            'artifact is not bound to this execution')
    item = matches[0]
    media_type = metadata['media_type']
    require(media_type in ('image/png', 'image/jpeg') and item['media_type'] == media_type,
            'unsupported or mismatched artifact media type')
    images = [c for c in artifact_result['content'] if c.get('type') == 'image']
    require(len(images) == 1 and images[0]['mimeType'] == media_type, 'missing artifact image block')
    data = images[0]['data']
    require(isinstance(data, str) and len(data) <= (MAX_BYTES + 2) // 3 * 4, 'artifact too large')
    payload = base64.b64decode(data, validate=True)
    digest = 'sha256:' + hashlib.sha256(payload).hexdigest()
    require(0 < len(payload) <= MAX_BYTES and len(payload) == metadata['size_bytes'] == item['size_bytes'],
            'artifact size mismatch')
    require(metadata['sha256'] == item['sha256'] == digest, 'artifact digest mismatch')
    require(payload.startswith(b'\x89PNG\r\n\x1a\n') if media_type == 'image/png'
            else payload.startswith(b'\xff\xd8\xff'), 'artifact file signature mismatch')
    summary = dict(execution_id=execution_id, artifact_id=metadata['artifact_id'],
                   authorization_id=authorization_id, settlement_id=settlement_id,
                   ledger_transaction_id=row['id'], provider=PROVIDERS[provider_id],
                   size=size, tier=tier, settled_cost=cost, sha256=digest,
                   cost_semantics='operation_total_not_per_image',
                   budget_charge_cents=receipt['budget_charge_cents'])
    suffix = 'png' if media_type == 'image/png' else 'jpg'
    return summary, payload, f'{PROVIDERS[provider_id]}-{size}-{tier}-{cost_label}c.{suffix}'


def atomic_write(path, data):
    require(not path.is_symlink(), 'refusing symlink output')
    fd, temporary = tempfile.mkstemp(prefix='.gallery-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def export(bundle, output):
    summary, payload, label = validated_export(bundle)
    output = Path(output)
    require(output.is_absolute() and not output.is_symlink(), 'gallery must be an absolute non-symlink directory')
    output.mkdir(parents=True, exist_ok=True)
    fd = os.open(output / '.gallery.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        manifest_path = output / '.gallery.json'
        require(not manifest_path.is_symlink(), 'refusing symlink manifest')
        manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else []
        key = (summary['execution_id'], summary['artifact_id'])
        existing = [entry for entry in manifest if (entry['execution_id'], entry['artifact_id']) == key]
        require(len(existing) <= 1, 'duplicate gallery manifest identity')
        if existing:
            entry = existing[0]
            require({k: entry[k] for k in summary} == summary, 'existing export evidence/labels changed')
        else:
            sequence = max((entry['sequence'] for entry in manifest), default=0) + 1
            entry = dict(summary, sequence=sequence, filename=f'{sequence:02d}-{label}')
            require(not (output / entry['filename']).exists(), 'refusing to overwrite unrelated gallery image')
            manifest.append(entry)
            # Reserve first: replay repairs interrupted writes without allocating a new sequence.
            atomic_write(manifest_path, (json.dumps(manifest, indent=2) + '\n').encode())
        path = output / entry['filename']
        require(path.name == entry['filename'], 'invalid manifest filename')
        if path.exists():
            require(not path.is_symlink() and hashlib.sha256(path.read_bytes()).hexdigest() == summary['sha256'][7:],
                    'existing gallery image changed')
        else:
            atomic_write(path, payload)
        atomic_write(path.with_suffix(path.suffix + '.receipt.json'), (json.dumps(entry, indent=2) + '\n').encode())
        return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', required=True, type=Path, help='local bundle of exact unified MCP results')
    parser.add_argument('--output', required=True, type=Path, help='absolute dedicated gallery directory')
    args = parser.parse_args()
    try:
        print(export(json.loads(args.input.read_text()), args.output))
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f'Gallery export refused: {error}\n')


if __name__ == '__main__':
    main()
