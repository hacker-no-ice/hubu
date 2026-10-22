"""Independent checks over saved public exports. No server, credentials or database access."""
from fractions import Fraction
import json
from pathlib import Path
import sys


class Evidence:
    def __init__(self, output):
        self.output = Path(output)
        self.checks = []
        if sys.flags.optimize:
            self.incomplete('Python assertion checks are enabled',
                            'Run without -O/-OO or PYTHONOPTIMIZE; optimized Python disables financial assertions.')

    def json(self, name):
        return json.loads((self.output / name).read_text())

    def jsonl(self, name):
        return [json.loads(line) for line in (self.output / name).read_text().splitlines()]

    def requests(self, label):
        records = [r for r in self.jsonl('requests.jsonl') if r['label'] == label]
        if not records:
            raise ValueError('Missing request evidence: ' + label)
        return records

    @staticmethod
    def money(value):
        if 'currency' in value and value['currency'].lower() != 'usd':
            raise ValueError('Unexpected currency')
        scale = value['scale']
        coefficient = value['units'] if 'units' in value else value['amount']
        if not isinstance(scale, int) or not 0 <= scale <= 18:
            raise ValueError('Invalid exact amount scale')
        if isinstance(coefficient, bool) or not isinstance(coefficient, (str, int)):
            raise ValueError('Exact amount requires an integer coefficient')
        return Fraction(int(coefficient), 10 ** scale)

    def check(self, name, predicate):
        try:
            passed = bool(predicate())
            result = dict(name=name, status='supported' if passed else 'contradicted')
        except AssertionError as error:
            result = dict(name=name, status='contradicted', detail=str(error))
        except (OSError, ValueError, KeyError, IndexError, TypeError) as error:
            result = dict(name=name, status='incomplete', detail=str(error))
        self.checks.append(result)
        return result['status'] == 'supported'

    def incomplete(self, name, detail):
        self.checks.append(dict(name=name, status='incomplete', detail=detail))

    def _pages(self, phase, kind, field):
        directory = self.output / 'snapshots' / phase
        paths = sorted(directory.glob(kind + '-*.json'))
        if not paths:
            raise ValueError('Missing ' + phase + '/' + kind + ' pages')
        records, ids, coverage = [], set(), None
        for index, path in enumerate(paths):
            if path.name != f'{kind}-{index + 1:03d}.json':
                raise ValueError('Non-contiguous export pages')
            page = json.loads(path.read_text())
            if page['schema_version'] != 'hubu-history-v1':
                raise ValueError('Unsupported public history schema')
            if (page['next_cursor'] is None) != (index == len(paths) - 1):
                raise ValueError('Incomplete or surplus pagination')
            for record in page[field]:
                if record['id'] in ids:
                    raise ValueError('Repeated immutable record across export pages')
                ids.add(record['id']); records.append(record)
            if kind == 'ledger':
                if coverage is not None and coverage != page['coverage']:
                    raise ValueError('Coverage changed while exporting paused writers')
                coverage = page['coverage']
        return records, coverage

    def snapshot(self, phase):
        transactions, coverage = self._pages(phase, 'ledger', 'transactions')
        authorizations, _ = self._pages(phase, 'authorizations', 'authorization_records')
        budgets = self.json('snapshots/' + phase + '/budgets.json')['budgets']
        if len(budgets) != 1:
            raise ValueError('Expected one isolated fixture budget')
        return dict(budget=budgets[0], transactions=transactions,
                    authorizations=authorizations, coverage=coverage)

    def fixed(self, initial='initial', final='final'):
        before, after = self.snapshot(initial), self.snapshot(final)
        for field in ('agent_id', 'budget_id', 'current_version_id', 'current_revision',
                      'amount_limit', 'starting_at', 'ending_before', 'currency'):
            assert before['budget'][field] == after['budget'][field], 'Configuration changed: ' + field
        a = (self.output / 'snapshots' / initial / 'policy.txt').read_text()
        b = (self.output / 'snapshots' / final / 'policy.txt').read_text()
        assert a == b, 'Public agent policy changed'
        return True

    def final(self, phase, *, count, consumed, frozen, remaining, pending):
        state = self.snapshot(phase)
        budget, coverage = state['budget'], state['coverage']
        txns, auths = state['transactions'], state['authorizations']
        assert len(txns) == count, f'{phase}: {len(txns)} postings, expected {count}'
        for field, expected in [('consumed_amount', consumed), ('frozen_amount', frozen), ('remaining_amount', remaining)]:
            assert self.money(budget[field]) == Fraction(expected), f'{phase}: {field}={budget[field]}'
        assert self.money(budget['amount_limit']) == sum(self.money(budget[k]) for k in
            ('consumed_amount', 'frozen_amount', 'remaining_amount')), 'Budget balance equation'
        assert coverage['budget_id'] == budget['budget_id'] and coverage['agent_id'] == budget['agent_id']
        assert self.money(coverage['consumed_amount']) == self.money(coverage['recorded_budget_charges']) == Fraction(consumed)
        assert self.money(coverage['unaccounted_consumption']) == 0, 'Unaccounted consumption'
        assert coverage['pending_hold_count'] == pending, 'Unexpected pending holds'
        assert coverage['owner_transactions_without_budget_context'] == 0, 'Unattributed transactions'
        assert sum(self.money(t['budget_charge_delta']) for t in txns) == Fraction(consumed), 'Signed control charges'
        by_auth = {a['id']: a for a in auths}
        assert len({t['authorization_id'] for t in txns}) == count, 'Repeated authorization posting'
        assert len({t['settlement_id'] for t in txns}) == count, 'Repeated settlement posting'
        for txn in txns:
            assert txn['kind'] == 'external_provider' and txn['cost_semantics'] == 'original_total'
            assert txn['original_transaction_id'] is None and txn['previous_transaction_id'] is None
            assert txn['budget_id'] == budget['budget_id'] and txn['budget_version_id'] == budget['current_version_id']
            assert txn['agent_id'] == budget['agent_id'] and txn['coverage']['has_budget_context']
            auth = by_auth[txn['authorization_id']]
            assert txn['account_id'] == auth['account_id'] and auth['status'] == 'settled'
            assert txn['claim_id'] == auth['claim']['id'] and txn['settlement_id'] == auth['receipt']['settlement_id']
            assert auth['ledger_transaction_ids'] == [txn['id']], 'Public receipt/ledger linkage'
            entries = txn['entries']
            assert len(entries) == 2, 'Unexpected ledger entry count'
            debit = [e for e in entries if e['direction'] == 'debit' and e['account_kind'] == 'AgentSpendExpense']
            credit = [e for e in entries if e['direction'] == 'credit' and e['account_kind'] == 'ExternallyBilledClearing']
            assert len(debit) == len(credit) == 1, 'Unexpected ledger direction/account'
            cost = self.money(txn['effective_cost'])
            assert self.money(debit[0]['amount']) == self.money(credit[0]['amount']) == cost
            assert cost == self.money(auth['receipt']['actual_vendor_cost']), 'Receipt/expense mismatch'
            assert self.money(txn['budget_charge_delta']) == self.money(auth['receipt']['budget_charge'])
            assert self.money(txn['budget_charge_delta']) - cost == Fraction(int(txn['rounding_delta_amount']), 10**18)
        return True

    def result(self, caveat=''):
        statuses = {c['status'] for c in self.checks}
        verdict = ('does not hold' if 'contradicted' in statuses else
                   'not tested' if 'incomplete' in statuses or not statuses else
                   'holds with caveat' if caveat else 'holds')
        return dict(verdict=verdict, checks=self.checks, caveat=caveat)


def budget_denial(record):
    if record['status'] is None or record['status'] >= 500:
        raise ValueError('Unresolved transport/server outcome is not a denial')
    body = record['body']
    return (record['status'] == 200 and body['decision'] == 'deny'
            and body['auth_token_id'] is None and body['budget_hold'] is None
            and any('budget' in reason.lower() for reason in body['reasons']))
