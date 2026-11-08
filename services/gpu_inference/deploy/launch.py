"""Same-process systemd credential bridge; the server's strict private-file check stays intact."""
from __future__ import annotations

import argparse
import contextlib
import os
import pathlib
import re
import stat
import sys
import tempfile

MAX_CREDENTIAL_BYTES = 1024


def _credential(directory: pathlib.Path, name: str) -> bytes:
    if not directory.is_absolute() or not re.fullmatch(r'[A-Za-z0-9_-][A-Za-z0-9_.-]{0,127}', name):
        raise ValueError('invalid systemd credential location')
    parent = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(parent)
        if info.st_uid not in (0, os.geteuid()) or info.st_mode & 0o027:
            raise ValueError('systemd credential directory is not protected')
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=parent)
        try:
            before = os.fstat(fd)
            if (not stat.S_ISREG(before.st_mode) or before.st_uid not in (0, os.geteuid())
                    or before.st_mode & 0o027 or not 0 < before.st_size <= MAX_CREDENTIAL_BYTES):
                raise ValueError('systemd credential is not a bounded protected regular file')
            value = bytearray()
            while len(value) <= MAX_CREDENTIAL_BYTES:
                chunk = os.read(fd, min(256, MAX_CREDENTIAL_BYTES + 1 - len(value)))
                if not chunk:
                    break
                value.extend(chunk)
            after = os.fstat(fd)
            current = os.stat(name, dir_fd=parent, follow_symlinks=False)
            fields = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns')
            if (len(value) > MAX_CREDENTIAL_BYTES
                    or any(getattr(before, key) != getattr(after, key) for key in fields)
                    or (current.st_dev, current.st_ino) != (before.st_dev, before.st_ino)):
                raise ValueError('systemd credential changed while reading')
        finally:
            os.close(fd)
    finally:
        os.close(parent)
    token = bytes(value).strip()
    if not 32 <= len(token) <= 256 or not token.isascii() or any(chr(byte).isspace() for byte in token):
        raise ValueError('invalid systemd credential value')
    return token


@contextlib.contextmanager
def private_token(directory: pathlib.Path, name: str, *, temp_root: str = '/tmp'):
    token = _credential(directory, name)
    # TemporaryDirectory is0700; O_EXCL and0600 protect the new inode even with a permissive umask.
    with tempfile.TemporaryDirectory(prefix='face-gpu-credential-', dir=temp_root) as folder:
        path = pathlib.Path(folder) / 'api-token'
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, 'wb', closefd=False) as stream:
                stream.write(token)
                stream.flush()
            os.fsync(fd)
        finally:
            os.close(fd)
        yield path
    # Normal return/error cleans the copy; SIGKILL cleanup is the unit's private tmpfs lifetime.


def run(server_args: list[str], *, credential_name: str = 'api-token', server_main=None):
    if any(value == '--token-file' or value.startswith('--token-file=') for value in server_args):
        raise ValueError('server token path is owned by the credential launcher')
    source = os.environ.get('CREDENTIALS_DIRECTORY')
    if not source:
        raise ValueError('systemd credential directory is required')
    with private_token(pathlib.Path(source), credential_name) as path:
        if server_main is None:
            from ..server import main as server_main
        previous = sys.argv
        try:
            sys.argv = [previous[0], *server_args, '--token-file', str(path)]
            return server_main()
        finally:
            sys.argv = previous


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--credential-name', default='api-token')
    parser.add_argument('server_args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    server_args = args.server_args[1:] if args.server_args[:1] == ['--'] else args.server_args
    run(server_args, credential_name=args.credential_name)


if __name__ == '__main__':
    main()
