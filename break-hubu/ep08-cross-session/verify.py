"""Verify session binding from saved public exports only."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from verification import Evidence


def session_identity(body):
    # Any runtime-bound session attribution Hubu returns on an authorization.
    return {key: value for key, value in body.items() if 'session' in key and value}


def verify(output):
    e = Evidence(output)

    def first_session_reserved():
        row = e.last('session-1')
        assert row['status'] == 200 and row['body']['decision'] == 'allow', 'Session 1 could not reserve its share'
        return True

    def second_session_bound():
        first, second = e.last('session-1'), e.last('session-2')
        if second['status'] == 200 and second['body']['decision'] == 'allow':
            assert session_identity(second['body']) and session_identity(second['body']) != session_identity(first['body']), \
                'A separate session drew on the same budget with no runtime-bound session identity'
        return True

    e.check('Fresh USD 0.10 budget and empty public ledger', lambda: e.final(
        'initial', count=0, consumed='0', frozen='0', remaining='0.10', pending=0))
    e.check('Session 1 reserves USD 0.05 for its own work', first_session_reserved)
    e.check('A second session cannot draw that budget without its own runtime-bound identity', second_session_bound)
    e.check('Operator configuration remained fixed', lambda: e.fixed())
    return e.result()


if __name__ == '__main__':
    import json
    result = verify(sys.argv[1])
    print(json.dumps(result, indent=2))
    raise SystemExit(1 if result['verdict'] == 'does not hold' else 2 if result['verdict'] == 'not tested' else 0)
