"""Public HTTP/CLI transport and process/export helpers; episode logic stays in attack.py."""
import argparse
from datetime import datetime, timezone
from decimal import Decimal
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from bootstrap import build

SCOPE = {'schema_version': 1, 'provider': 'provider:local:fixture', 'executor': 'executor:gongbu:image',
         'capability': 'capability:image:generate', 'billing_merchant': 'merchant:local'}
POLICY = '''id: break_hubu_fixture
version: v1
default_effect: deny
rules:
  - id: allow_synthetic_fixture
    effect: allow
    reason: synthetic local fixture only
    when:
      op: eq
      field: provider
      value:
        string: provider:local:fixture
'''


def now():
    return datetime.now(timezone.utc).isoformat()


def save(path, value):
    path = Path(path); path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('w') as stream:
        json.dump(value, stream, indent=2); stream.write('\n'); stream.flush(); os.fsync(stream.fileno())


def append(path, value):
    # One append write per record, including concurrent EP04 worker processes.
    data = (json.dumps(value, separators=(',', ':')) + '\n').encode()
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, data)
    finally:
        os.close(fd)


def redact(value):
    private = {'operation_key', 'auth_token_id', 'spend_auth_token_id', 'session_token'}
    if isinstance(value, dict):
        return {k: ('[redacted]' if k in private and v is not None else redact(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    return value


class Runtime:
    def __init__(self, context, owner=False):
        self.context = context
        self.output = Path(context['output'])
        self.private = Path(context['private'])
        self.ids = context.get('ids', {})
        self.url = context['url']
        self.owner = owner
        self.server = None

    def note(self, text):
        with (self.output / 'transcript.txt').open('a') as stream:
            stream.write(f'[{now()}] {text}\n')

    def environment(self):
        return {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'HOME': str(self.private / 'home'),
                'HUBU_HOME': str(self.private / 'hubu-home'), 'NO_COLOR': '1', 'TERM': 'dumb',
                'PYTHONDONTWRITEBYTECODE': '1',
                'HUBU_AUTH_TOKEN_FILE': self.context['auth_file']}

    def cli(self, *args, owner=False, check=True):
        command = [str(Path(self.context['binary_directory']) / 'hubu'), '--url', self.url, *map(str, args)]
        env = self.environment()
        if owner:
            if not self.owner: raise RuntimeError('Owner capability belongs to the parent only')
            env['HUBU_RECONCILIATION_TOKEN_FILE'] = str(self.private / 'owner-reconciliation.token')
        self.note('$ ' + shlex.join(command))
        result = subprocess.run(command, env=env, cwd=self.private, capture_output=True, text=True, timeout=60)
        self.note(result.stdout.rstrip() + ('\n' + result.stderr.rstrip() if result.stderr else '') + f'\nCLI exit={result.returncode}')
        if check and result.returncode: raise RuntimeError(f'CLI failed: {shlex.join(args)}: {result.stderr}')
        return {'exit_code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}

    def request(self, method, route, body=None, label='request', retry=True):
        encoded = None if body is None else json.dumps(body, separators=(',', ':')).encode()
        request_file = self.private / 'requests' / (secrets.token_hex(12) + '.json')
        durable = dict(label=label, method=method, route=route, body=body)
        save(request_file, durable)
        headers = {'Authorization': 'Bearer ' + Path(self.context['auth_file']).read_text().strip(),
                   'Content-Type': 'application/json'}
        # Exact persisted body is reused on ambiguous transport responses.
        for attempt in range(1, 4 if retry else 2):
            started = now()
            try:
                request = urllib.request.Request(self.url + route, encoded, headers, method=method)
                with urllib.request.urlopen(request, timeout=15) as response:
                    result = {'status': response.status, 'body': json.loads(response.read())}
            except urllib.error.HTTPError as error:
                raw = error.read().decode()
                try: payload = json.loads(raw)
                except json.JSONDecodeError: payload = {'error': raw}
                result = {'status': error.code, 'body': payload}
            except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
                result = {'status': None, 'body': None, 'transport_error': str(error)}
            record = dict(type='http', label=label, attempt=attempt, started_at=started, ended_at=now(),
                          method=method, route=route, request=redact(body), **redact(result))
            save(request_file, dict(**durable, last_attempt=attempt, response=result))
            append(self.output / 'requests.jsonl', record)
            self.note(f'{label} {method} {route} -> {result["status"]}: ' + json.dumps(redact(result['body'])))
            if result['status'] is not None and result['status'] < 500: return result
            if attempt < (3 if retry else 1): time.sleep(0.2 * attempt)
        return result

    def authorize(self, label, amount):
        path = self.private / 'operations' / (label + '.json')
        if path.exists():
            body = json.loads(path.read_text())
            if body['amount'] != amount: raise ValueError('Retry changes amount')
        else:
            body = dict(operation_key='break-hubu:' + secrets.token_hex(24), account_id=self.ids['account_id'],
                        amount=amount, currency='usd', reason='Synthetic proof kit: ' + label,
                        execution_scope=SCOPE, lease_profile=self.context['lease_profile'])
            save(path, body)
        return self.request('POST', '/spend/authorize', body, label=label)

    def claim(self, authorization, label='claim'):
        token = authorization['body']['auth_token_id']
        resolved = self.request('POST', '/spend/executor/resolve', {'spend_auth_token_id': token}, label=label + '-resolve')
        if resolved['status'] != 200: return resolved
        body = dict(spend_auth_token_id=token, contract_version='hubu-spend-executor-v4.5',
                    **{key: resolved['body'][key] for key in ('account_id', 'amount', 'execution_scope')})
        return self.request('POST', '/spend/executor/claim', body, label=label)

    def receipt(self, label, amount):
        units = Decimal(amount) * 1000000
        if units != units.to_integral_value(): raise ValueError('Fixture cost exceeds six decimal places')
        receipt = {'actual_vendor_cost': {'amount': int(units), 'scale': 6, 'currency': 'usd'},
                   'provider_request_id': 'synthetic:' + label,
                   'price_model_snapshot': {'provider': SCOPE['provider'], 'model': 'synthetic-no-provider',
                                            'pricing_unit': 'operation', 'currency': 'usd', 'synthetic': True},
                   'artifact_reference': 'fixture://' + self.context['episode'] + '/' + label}
        self.write_fixture(label + '-receipt.json', receipt)
        return receipt

    def write_fixture(self, name, value):
        save(self.output / 'fixtures' / name, value)
        append(self.output / 'process-events.jsonl', dict(event='fixture_saved', at=now(), path='fixtures/' + name))
        self.note('Durable synthetic fixture (not provider evidence): fixtures/' + name)

    def settle(self, claim, receipt, label='settle'):
        return self.request('POST', '/spend/executor/settle', {'claim_id': claim['body']['claim_id'], 'receipt': receipt}, label=label)

    def release(self, claim, label='release'):
        return self.request('POST', '/spend/executor/release', {'claim_id': claim['body']['claim_id']}, label=label)

    def inspect(self, authorization_id):
        result = self.cli('spend', 'show', '--authorization-id', authorization_id)
        return {'status': 200, 'body': json.loads(result['stdout'])}

    def wait_expiry(self, claim):
        body = claim['body']; deadline = datetime.fromisoformat(body['claim_expires_at'].replace('Z', '+00:00')).timestamp()
        while time.time() <= deadline + 15:
            result = self.request('GET', '/spend/executor/claim?claim_id=' + body['claim_id'], label='poll-expiry')
            if result['status'] == 200 and result['body'].get('reconciliation_required'):
                self.request('GET', '/spend/executor/reconciliation', label='reconciliation-visible')
                return result
            time.sleep(min(1, max(0.1, deadline - time.time())))
        raise RuntimeError('Public claim did not require reconciliation by returned deadline + 15s')

    def snapshot(self, phase):
        directory = self.output / 'snapshots' / phase
        directory.mkdir(parents=True, exist_ok=False)
        self.note('PUBLIC SNAPSHOT ' + phase + ' (writers paused)')
        budgets = self.request('GET', '/budgets', label='snapshot-' + phase + '-budgets')
        if budgets['status'] != 200: raise RuntimeError('Budget export failed')
        save(directory / 'budgets.json', budgets['body'])
        (directory / 'budget.txt').write_text(self.cli('budget', 'list')['stdout'])
        (directory / 'policy.txt').write_text(self.cli('policy', 'show', '--agent-id', self.ids['agent_id'])['stdout'])
        for kind, command in [('ledger', ['ledger', 'list']), ('authorizations', ['spend', 'authorizations'])]:
            cursor, seen = None, set()
            for page in range(1, 1001):
                args = command + ['--agent-id', self.ids['agent_id'], '--limit', '100']
                if kind == 'ledger': args += ['--budget-id', self.ids['budget_id']]
                if cursor: args += ['--cursor', cursor]
                result = self.cli(*args)
                payload = json.loads(result['stdout'])
                # Preserve CLI's original public JSON envelope, without transformations.
                (directory / f'{kind}-{page:03d}.json').write_text(result['stdout'])
                cursor = payload.get('next_cursor')
                if cursor is None: break
                if cursor in seen: raise RuntimeError('Public history pagination repeated its cursor')
                seen.add(cursor)
            else: raise RuntimeError('Public history pagination exceeded bound')
        return directory

    def control(self, action, **kwargs):
        if self.owner: return self._control(action, **kwargs)
        print(json.dumps(dict(action=action, **kwargs)), flush=True)
        response = json.loads(sys.stdin.readline())
        if 'error' in response: raise RuntimeError(response['error'])
        return response['result']

    def owner_reconcile(self, claim, receipt, evidence='synthetic receipt fixture', label='owner-reconcile'):
        return self.control('reconcile', claim=claim, receipt=receipt, evidence=evidence, label=label)

    def kill(self): return self.control('kill')
    def restart(self): return self.control('restart')

    def _control(self, action, **kwargs):
        if action == 'kill':
            if self.server is None or self.server.poll() is not None: raise RuntimeError('Owned server is not running')
            pid = self.server.pid; self.note(f'FAULT: SIGKILL owned Hubu server PID {pid}')
            self.server.kill(); code = self.server.wait(timeout=10)
            append(self.output / 'process-events.jsonl', dict(event='kill', at=now(), pid=pid, signal='SIGKILL', exit_code=code))
            if code != -signal.SIGKILL: raise RuntimeError('Server did not exit due to the requested SIGKILL')
            return {'pid': pid, 'exit_code': code}
        if action == 'restart':
            if self.server and self.server.poll() is None: self.server.terminate(); self.server.wait(timeout=10)
            command = [str(Path(self.context['binary_directory']) / 'hubu-server'), 'serve', '--config', str(self.private / 'server.json')]
            self.note('$ ' + shlex.join(command))
            with (self.private / 'server.log').open('a') as log:
                self.server = subprocess.Popen(command, cwd=self.private, env=self.environment(), stdout=log, stderr=log)
            for _ in range(100):
                if self.server.poll() is not None: raise RuntimeError('Hubu startup failed; inspect private server.log')
                try:
                    with urllib.request.urlopen(self.url + '/health', timeout=0.3) as response:
                        if response.status == 200: break
                except (urllib.error.URLError, TimeoutError): time.sleep(0.1)
            else: raise RuntimeError('Hubu health deadline exceeded')
            event = dict(event='restart', at=now(), pid=self.server.pid)
            append(self.output / 'process-events.jsonl', event); return event
        if action == 'reconcile':
            claim, receipt = kwargs['claim'], kwargs['receipt']; cost = receipt['actual_vendor_cost']
            self.note('OWNER STEP: explicit reconciliation of a synthetic known bill; capability stays in parent')
            result = self.cli('spend', 'reconcile', 'billed', '--claim-id', claim['body']['claim_id'],
                              '--provider-reference', receipt['provider_request_id'], '--evidence', kwargs['evidence'],
                              '--actual-vendor-cost-amount', cost['amount'], '--actual-vendor-cost-scale', cost['scale'],
                              '--currency', cost['currency'], '--provider-request-id', receipt['provider_request_id'],
                              '--price-model-snapshot-json', json.dumps(receipt['price_model_snapshot']),
                              '--artifact-reference', receipt['artifact_reference'], owner=True, check=False)
            append(self.output / 'requests.jsonl', dict(type='owner_cli', label=kwargs['label'], at=now(),
                   claim_id=claim['body']['claim_id'], receipt=receipt, evidence=kwargs['evidence'], **result))
            return result
        raise ValueError('Unknown parent operation')


def run_episode(attack, verify, *, episode, budget, short_lease=False):
    parser = argparse.ArgumentParser(description=episode + ': unchanged released Hubu, synthetic executor')
    parser.add_argument('--release', default='v0.2.4'); parser.add_argument('--output'); parser.add_argument('--cache')
    parser.add_argument('--attack-context', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.attack_context:
        attack(Runtime(json.loads(Path(args.attack_context).read_text())))
        return
    output = Path(args.output).resolve() if args.output else Path(tempfile.mkdtemp(prefix=episode + '-evidence-'))
    if output.exists() and any(output.iterdir()): raise RuntimeError('Output must be a fresh empty directory')
    output.mkdir(parents=True, exist_ok=True)
    private = Path(tempfile.mkdtemp(prefix=episode + '-private-')).resolve()
    (private / 'home').mkdir(); (private / 'hubu-home').mkdir()
    context = dict(output=str(output), private=str(private), episode=episode,
                   lease_profile='proof_short' if short_lease else 'default')
    rt = None; child = None; started = time.monotonic(); error = None
    print('Evidence:', output, flush=True)
    try:
        metadata = build(args.release, args.cache)
        context.update(metadata)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0)); port = listener.getsockname()[1]
        context['url'] = f'http://127.0.0.1:{port}'
        for name in ('auth', 'approval', 'owner-reconciliation'):
            path = private / (name + '.token'); path.write_text(secrets.token_urlsafe(48) + '\n'); path.chmod(0o600)
        context['auth_file'] = str(private / 'auth.token')
        lease = private / 'leases.yaml'
        lease.write_text('authorization_ttl_seconds: 300\ndefault_lease_profile: default\nlease_profiles:\n  default:\n    claim_ttl_seconds: 900\n  proof_short:\n    claim_ttl_seconds: 30\n')
        config = dict(schema_version=1, listen=f'127.0.0.1:{port}', database_path=str(private / 'hubu.sqlite'),
                      auth_token_file=context['auth_file'], approval_token_file=str(private / 'approval.token'),
                      reconciliation_token_file=str(private / 'owner-reconciliation.token'), lease_config=str(lease))
        save(private / 'server.json', config)
        rt = Runtime(context, owner=True)
        rt.note('Release build: ' + shlex.join(metadata['build_command']))
        rt.note('Verified binary versions: ' + json.dumps(metadata['binary_versions']))
        command = [str(Path(metadata['binary_directory']) / 'hubu-server'), 'validate-config', '--config', str(private / 'server.json')]
        rt.note('$ ' + shlex.join(command))
        validation = subprocess.run(command, env=rt.environment(), capture_output=True, text=True, check=True)
        rt.note(validation.stdout)
        rt.restart()
        rt.cli('register', 'human', '--username', 'proof-kit-owner', '--display-name', 'Synthetic proof-kit owner')
        registration = rt.cli('register', 'agent', '--name', episode, '--version', args.release)['stdout']
        context['ids'] = {key: re.search(r'\b' + key + r':\s*(\S+)', registration).group(1)
                          for key in ('agent_id', 'account_id')}
        rt.ids = context['ids']
        policy = private / 'policy.yaml'; policy.write_text(POLICY)
        rt.cli('policy', 'validate', '--path', policy)
        rt.cli('policy', 'apply', '--path', policy, '--name', 'Synthetic fixture allow policy', '--agent-id', rt.ids['agent_id'])
        rt.cli('budget', 'create', '--agent-id', rt.ids['agent_id'], '--amount', budget,
               '--currency', 'usd', '--ending-before', '2999-01-01T00:00:00Z')
        budgets = rt.request('GET', '/budgets', label='setup-budget')['body']['budgets']
        if len(budgets) != 1: raise RuntimeError('Fresh fixture must have exactly one budget')
        rt.ids['budget_id'] = budgets[0]['budget_id']
        guidance = rt.request('GET', '/.well-known/hubu-spend-executor.json', label='executor-guidance')
        save(output / 'executor-guidance.json', guidance['body'])
        if guidance['status'] != 200 or guidance['body']['lease']['lease_profiles'][context['lease_profile']]['claim_ttl_seconds'] != (30 if short_lease else 900):
            raise RuntimeError('Public executor guidance did not confirm configured claim lease')
        kits_root = Path(__file__).resolve().parent.parent
        kits_commit = subprocess.check_output(['git', '-C', str(kits_root), 'rev-parse', 'HEAD'], text=True).strip()
        dirty = bool(subprocess.check_output(['git', '-C', str(kits_root), 'status', '--porcelain'], text=True).strip())
        save(output / 'run.json', dict(**metadata, episode=episode, budget=budget, currency='usd',
             lease_profile=context['lease_profile'], claim_ttl_seconds=30 if short_lease else 900,
             kits_commit=kits_commit, kits_worktree_dirty=dirty, platform=platform.platform(), machine=platform.machine(),
             python=sys.version, started_at=now(), ids=rt.ids, invocation=sys.argv))
        child_context = private / 'attack-context.json'; save(child_context, context)
        rt.snapshot('initial')
        command = [sys.executable, str(Path(sys.argv[0]).resolve()), '--attack-context', str(child_context)]
        error_path = private / 'attack-stderr.txt'
        with error_path.open('w') as errors:
            child = subprocess.Popen(command, cwd=private, env=rt.environment(), stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=errors, text=True, bufsize=1, start_new_session=True)
            for line in child.stdout:
                request = json.loads(line)
                try: response = {'result': rt._control(**request)}
                except Exception as exc: response = {'error': str(exc)}
                child.stdin.write(json.dumps(response) + '\n'); child.stdin.flush()
            code = child.wait()
        stderr = error_path.read_text()
        if stderr: rt.note(stderr)
        if code: raise RuntimeError(f'Attack process exited {code}: {stderr}')
    except (Exception, KeyboardInterrupt) as exc:
        error = str(exc) or 'Interrupted by operator'
        if rt: rt.note('INCOMPLETE: ' + error)
        else: (output / 'transcript.txt').write_text('INCOMPLETE SETUP: ' + error + '\n')
        if not (output / 'run.json').exists():
            save(output / 'run.json', dict(release=args.release, episode=episode, budget=budget,
                 platform=platform.platform(), python=sys.version, invocation=sys.argv, setup_error=error))
    finally:
        if child and child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try: child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL); child.wait(timeout=3)
        if rt and rt.server and rt.server.poll() is None:
            rt.server.terminate(); rt.server.wait(timeout=10)
    try:
        result = verify(output)
    except Exception as exc:
        result = {'verdict': 'not tested', 'checks': [], 'verification_error': str(exc)}
    if result.get('verdict') not in ('holds', 'holds with caveat', 'does not hold', 'not tested'):
        result = {'verdict': 'not tested', 'checks': [], 'verification_error': 'Verifier returned an unknown verdict'}
    result.update(episode=episode, execution='incomplete' if error else 'complete', elapsed_seconds=round(time.monotonic() - started, 3))
    if error:
        result['execution_error'] = error
        if result.get('verdict') not in ('does not hold',): result['verdict'] = 'not tested'
    save(output / 'result.json', result)
    (output / 'verification.txt').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2)); print('Private runtime (do not publish):', private)
    sys.exit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
