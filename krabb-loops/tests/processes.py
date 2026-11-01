"""Exercise real process termination; fixtures never call an AI provider."""

import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest

import legacy


ROOT = Path(__file__).resolve().parents[2]

CHILD = '''import os, signal
from pathlib import Path
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pid = Path(os.environ.get('FIXTURE', '.')) / 'child.pid'
staged = pid.with_suffix('.pending')
staged.write_text(str(os.getpid()))
staged.replace(pid)
while True:
    signal.pause()
'''

PARENT = '''import subprocess, sys, signal
subprocess.Popen([sys.executable, '-c', sys.argv[1]], start_new_session=True)
while True:
    signal.pause()
'''


def ready(path, process):
    deadline = time.monotonic() + 10

    while not path.exists():
        if process.poll() is not None:
            raise AssertionError(f'fixture exited before readiness: {process.returncode}')

        if time.monotonic() >= deadline:
            raise AssertionError('fixture readiness timeout')

        time.sleep(0.01)

    return int(path.read_text())


class Supervisor(unittest.TestCase):
    def test_pid_is_not_visible_until_publication_completes(self):
        gate = '''import sys
from pathlib import Path
write = Path.write_text
def staged(path, text):
    path.touch()
    print('created', flush=True)
    sys.stdin.readline()
    return write(path, text)
Path.write_text = staged
'''

        with tempfile.TemporaryDirectory(prefix='krabb publication ') as directory:
            process = subprocess.Popen([sys.executable, '-c', gate + CHILD], cwd=directory,
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

            try:
                readable, _, _ = select.select([process.stdout], [], [], 5)

                self.assertTrue(readable, 'fixture did not reach the publication gate')
                self.assertEqual(process.stdout.readline(), 'created\n')

                path = Path(directory) / 'child.pid'

                self.assertFalse(path.exists(), 'readiness exposed an incomplete PID')

                process.stdin.write('\n')
                process.stdin.flush()

                self.assertEqual(ready(path, process), process.pid)
            finally:
                process.kill()
                process.communicate(timeout=3)

    def test_timeout_reaps_detached_child(self):
        with tempfile.TemporaryDirectory(prefix='krabb process ') as directory:
            process = subprocess.Popen([sys.executable, str(ROOT / 'krabb-loops/lib/process.py'),
                                        '1s', sys.executable, '-c', PARENT, CHILD], cwd=directory)

            self.addCleanup(lambda: process.kill() if process.poll() is None else None)

            pid = ready(Path(directory) / 'child.pid', process)

            self.assertEqual(process.wait(timeout=6), 124)
            self.assertFalse(Path(f'/proc/{pid}').exists())

    def test_interrupt_reaps_detached_child(self):
        with tempfile.TemporaryDirectory(prefix='krabb process ') as directory:
            process = subprocess.Popen([sys.executable, str(ROOT / 'krabb-loops/lib/process.py'),
                                        'loop', sys.executable, '-c', PARENT, CHILD], cwd=directory)

            self.addCleanup(lambda: process.kill() if process.poll() is None else None)

            pid = ready(Path(directory) / 'child.pid', process)

            process.send_signal(signal.SIGTERM)

            self.assertEqual(process.wait(timeout=6), 143)
            self.assertFalse(Path(f'/proc/{pid}').exists())

    def test_success_also_reaps_background_children(self):
        with tempfile.TemporaryDirectory(prefix='krabb process ') as directory:
            command = '''import subprocess, sys
from pathlib import Path
import time
subprocess.Popen([sys.executable, '-c', sys.argv[1]], start_new_session=True)
while not Path('child.pid').exists():
    time.sleep(0.01)
'''

            result = subprocess.run([sys.executable, str(ROOT / 'krabb-loops/lib/process.py'),
                                     '5s', sys.executable, '-c', command, CHILD], cwd=directory,
                                    timeout=7)

            self.assertEqual(result.returncode, 0)

            pid = int((Path(directory) / 'child.pid').read_text())

            self.assertFalse(Path(f'/proc/{pid}').exists())


class LoopInterruption(unittest.TestCase):
    setUp = legacy.LegacyContract.setUp
    git = legacy.LegacyContract.git
    run_loop = legacy.LegacyContract.run_loop
    calls = legacy.LegacyContract.calls

    def hang_agent(self):
        path = self.root / 'bin/codex'
        source = path.read_text()
        marker = "    mode = args[args.index('--sandbox') + 1]"

        insertion = '''    import subprocess, signal
    subprocess.Popen([sys.executable, '-c', os.environ['CHILD_CODE']], start_new_session=True)
    while True:
        signal.pause()
'''

        # Insert after help handling, before any model response is produced.
        source = source.replace("if name in ('codex', 'claude'):\n",
                                "if name in ('codex', 'claude'):\n" + insertion)

        path.write_text(source)

        self.env['CHILD_CODE'] = CHILD

        (self.fixture / 'reports.json').write_text(json.dumps([legacy.CLEAR]))

    def test_agent_timeout_is_non_success_and_stops_children(self):
        self.hang_agent()

        self.env['KRABB_REVLOOP_TIMEOUT'] = '1s'
        result = self.run_loop()

        self.assertEqual(result.returncode, 70, result.stderr)

        pid = int((self.fixture / 'child.pid').read_text())

        self.assertFalse(Path(f'/proc/{pid}').exists())

    def test_termination_preserves_outcome_and_stops_children(self):
        self.hang_agent()

        output = (self.root / 'process.log').open('w')

        self.addCleanup(output.close)

        process = subprocess.Popen(['bash', str(self.root / 'krabb-loops/revloop.sh')],
                                   cwd=self.root, env=self.env, stdout=output, stderr=output)

        self.addCleanup(lambda: process.kill() if process.poll() is None else None)

        pid = ready(self.fixture / 'child.pid', process)

        process.terminate()

        self.assertEqual(process.wait(timeout=6), 143)
        self.assertFalse(Path(f'/proc/{pid}').exists())

        state = self.root / '.krabb/loops/rev'
        run = Path((state / 'latest-run.txt').read_text().strip())

        self.assertIn('exit=143', (run / 'outcome.txt').read_text())


if __name__ == '__main__':
    unittest.main(verbosity=2)
