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


def _systemd_directory(directory: pathlib.Path) -> int:
    # This privileged mount is supplied by the system manager, never an arbitrary
    # absolute path advertised through an environment variable.
    if (directory.parent != pathlib.Path('/run/credentials')
            or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.@:-]{0,200}\.service', directory.name)):
        raise ValueError('a system-manager credential directory is required')
    flags = getattr(os, 'O_PATH', os.O_RDONLY) | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    held = os.open('/', flags)
    try:
        for component in ('run', 'credentials', directory.name):
            child = os.open(component, flags, dir_fd=held)
            os.close(held)
            held = child
            info = os.fstat(held)
            if info.st_uid != 0 or info.st_mode & 0o022:
                raise ValueError('systemd credential ancestors must be protected and root-owned')
        info = os.fstat(held)
        if info.st_mode & 0o027 or not os.fstatvfs(held).f_flag & os.ST_RDONLY:
            raise ValueError('systemd credential directory must be protected and read-only')
        with open('/proc/self/mountinfo', encoding='utf-8') as stream:
            mounts = stream.read(1024 * 1024 + 1)
        if len(mounts) > 1024 * 1024:
            raise ValueError('credential mount inventory exceeds bound')
        matches = []
        for line in mounts.splitlines():
            left, separator, right = line.partition(' - ')
            fields = left.split()
            if separator and len(fields) >= 6 and fields[4] == str(directory):
                details = right.split()
                if len(details) >= 3:
                    matches.append((fields, details))
        if len(matches) != 1:
            raise ValueError('exact systemd credential mount is required')
        fields, details = matches[0]
        options = set(fields[5].split(',')) | set(details[2].split(','))
        if (details[0] != 'tmpfs' or fields[2] != f'{os.major(info.st_dev)}:{os.minor(info.st_dev)}'
                or not {'ro', 'nosuid', 'nodev', 'noexec'}.issubset(options)):
            raise ValueError('protected credential tmpfs mount is required')
        current = os.stat(directory, follow_symlinks=False)
        if (current.st_dev, current.st_ino) != (info.st_dev, info.st_ino):
            raise ValueError('credential directory changed')
        return held
    except BaseException:
        os.close(held)
        raise


def _read_token(parent: int, name: str, expected_uid: int) -> bytes:
    # A narrow filesystem seam for real-file tests. Production always supplies
    # the verified system-manager descriptor and root UID; no CLI override exists.
    if not re.fullmatch(r'[A-Za-z0-9_-][A-Za-z0-9_.-]{0,127}', name):
        raise ValueError('invalid systemd credential name')
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=parent)
    try:
        before = os.fstat(fd)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != expected_uid
                or stat.S_IMODE(before.st_mode) not in (0o400, 0o440)
                or not 0 < before.st_size <= MAX_CREDENTIAL_BYTES):
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
    token = bytes(value).strip()
    if not 32 <= len(token) <= 256 or not token.isascii() or any(chr(byte).isspace() for byte in token):
        raise ValueError('invalid systemd credential value')
    return token


def _systemd_credential(directory: pathlib.Path, name: str) -> bytes:
    parent = _systemd_directory(directory)
    try:
        return _read_token(parent, name, 0)
    finally:
        os.close(parent)


@contextlib.contextmanager
def _private_token(token: bytes, *, temp_root: str = '/tmp'):
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



@contextlib.contextmanager
def private_token(credential_name: str = 'api-token'):
    source = os.environ.get('CREDENTIALS_DIRECTORY')
    if not source:
        raise ValueError('systemd credential directory is required')
    token = _systemd_credential(pathlib.Path(source), credential_name)
    with _private_token(token) as path:
        yield path

def run(server_args: list[str], *, credential_name: str = 'api-token', server_main=None):
    if any(value == '--token-file' or value.startswith('--token-file=') for value in server_args):
        raise ValueError('server token path is owned by the credential launcher')
    with private_token(credential_name) as path:
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
