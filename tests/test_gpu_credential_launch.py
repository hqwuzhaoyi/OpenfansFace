"""Credential bridge only: real files, no credential output, no models/services."""
import contextlib
import io
import os
from pathlib import Path
import stat
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from services.gpu_inference.deploy.launch import private_token, _private_token, _read_token, _systemd_directory, run

TOKEN = b'fixture-credential-not-production-' + b'x' * 24

class CredentialLauncherTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='face-gpu-credential-test-')
        self.root = Path(self.tmp.name)
        self.source = self.root / 'api-token'
        self.source.write_bytes(TOKEN)
        self.source.chmod(0o440)
    @contextlib.contextmanager
    def copied_fixture(self, name='api-token'):
        # Filesystem-only seam: production does NOT accept this temporary path.
        fd=os.open(self.root,os.O_RDONLY|os.O_DIRECTORY)
        try: token=_read_token(fd,name,os.geteuid())
        finally: os.close(fd)
        with _private_token(token) as path:yield path
    def tearDown(self):
        self.root.chmod(0o700)
        self.tmp.cleanup()
    def test_0440_source_copies_to_0600_exclusive_private_file_then_cleans(self):
        self.root.chmod(0o550)
        with self.copied_fixture() as path:
            saved = path
            self.assertEqual(path.read_bytes(), TOKEN)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(path.parent.stat().st_mode), 0o700)
            self.assertEqual(path.stat().st_uid, os.geteuid())
        self.assertFalse(saved.exists()); self.assertFalse(saved.parent.exists())
        self.assertEqual(self.source.read_bytes(), TOKEN)
        self.assertEqual(stat.S_IMODE(self.source.stat().st_mode), 0o440)
    def test_same_process_preserves_server_arguments_and_emits_no_secret(self):
        captured = []; previous = sys.argv; pid = os.getpid(); output = io.StringIO()
        def server():
            self.assertEqual(os.getpid(), pid)
            self.assertEqual(sys.argv[1:3], ['--host','127.0.0.1'])
            path=Path(sys.argv[sys.argv.index('--token-file')+1]);captured.append(path)
            self.assertEqual(path.stat().st_mode & 0o077, 0)  # unchanged server guard
            self.assertEqual(path.read_bytes(), TOKEN)
            return 'fixture result'
        with patch.dict(os.environ, {'CREDENTIALS_DIRECTORY':'/run/credentials/face-gpu.service'}), patch('services.gpu_inference.deploy.launch._systemd_credential',return_value=TOKEN), contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            self.assertEqual(run(['--host','127.0.0.1'], server_main=server), 'fixture result')
        self.assertIs(sys.argv, previous);self.assertEqual(output.getvalue(),'')
        self.assertFalse(captured[0].exists());self.assertEqual(self.source.read_bytes(),TOKEN)
    def test_server_failure_cleans_private_copy(self):
        saved=[]
        def fail():
            saved.append(Path(sys.argv[-1]));raise RuntimeError('fixture failure')
        with patch.dict(os.environ, {'CREDENTIALS_DIRECTORY':'/run/credentials/face-gpu.service'}), patch('services.gpu_inference.deploy.launch._systemd_credential',return_value=TOKEN):
            with self.assertRaisesRegex(RuntimeError,'fixture failure'):run([],server_main=fail)
        self.assertFalse(saved[0].exists());self.assertFalse(saved[0].parent.exists())
    def test_oversize_and_invalid_values_are_rejected_without_output(self):
        for value in (b'x'*1025,b'short',b'x'*32+b' '+b'y'*32,b'\xff'*40):
            with self.subTest(size=len(value)):
                self.source.chmod(0o600);self.source.write_bytes(value);self.source.chmod(0o440);out=io.StringIO()
                with contextlib.redirect_stdout(out),contextlib.redirect_stderr(out),self.assertRaises(ValueError):
                    with self.copied_fixture():pass
                self.assertEqual(out.getvalue(),'')
    def test_symlink_fifo_and_permissive_source_are_rejected_promptly(self):
        self.source.unlink();self.source.symlink_to(self.root/'other')
        with self.assertRaises(OSError):
            with self.copied_fixture():pass
        self.source.unlink();os.mkfifo(self.source);started=time.monotonic()
        with self.assertRaises(ValueError):
            with self.copied_fixture():pass
        self.assertLess(time.monotonic()-started,1)
        self.source.unlink();self.source.write_bytes(TOKEN);self.source.chmod(0o644)
        with self.assertRaises(ValueError):
            with self.copied_fixture():pass
    def test_parent_and_names_are_closed(self):
        for name in ('../api-token','/api-token','..'):
            with self.assertRaises(ValueError):
                with self.copied_fixture(name):pass
        for directory in (self.root,Path('/tmp/face-gpu.service'),Path('/run/credentials/../outside.service'),Path('/run/credentials/x/y.service')):
            with self.subTest(directory=directory),self.assertRaises(ValueError):_systemd_directory(directory)
        called=[]
        with patch.dict(os.environ,{'CREDENTIALS_DIRECTORY':str(self.root)}),self.assertRaises(ValueError):run([],server_main=lambda:called.append(True))
        self.assertEqual(called,[])
    def test_production_context_refuses_arbitrary_absolute_directory(self):
        with patch.dict(os.environ,{'CREDENTIALS_DIRECTORY':str(self.root)}),self.assertRaises(ValueError):
            with private_token():self.fail('untrusted directory was accepted')
        self.assertEqual(self.source.read_bytes(),TOKEN)
    def test_expected_owner_is_required_by_bounded_reader(self):
        fd=os.open(self.root,os.O_RDONLY|os.O_DIRECTORY)
        try:
            with self.assertRaises(ValueError):_read_token(fd,'api-token',os.geteuid()+1)
        finally:os.close(fd)
    def test_replaced_source_cannot_be_published(self):
        original=os.read;replaced=False
        def read(fd,size):
            nonlocal replaced
            data=original(fd,size)
            if not replaced:
                replaced=True;other=self.root/'replacement';other.write_bytes(TOKEN);other.chmod(0o440);other.replace(self.source)
            return data
        with patch('services.gpu_inference.deploy.launch.os.read',side_effect=read),self.assertRaises(ValueError):
            with self.copied_fixture():pass
    def test_caller_cannot_replace_private_destination_and_systemd_env_required(self):
        for args in (['--token-file','/elsewhere'],['--token-file=/elsewhere']):
            with self.assertRaises(ValueError):run(args,server_main=lambda:None)
        with patch.dict(os.environ,{},clear=True),self.assertRaises(ValueError):run([],server_main=lambda:None)

if __name__=='__main__':unittest.main()
