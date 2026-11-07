"""Authenticated image-only HTTP adapter. Start manually in an isolated environment.

No file/database mutation API, arbitrary resource fetch, shell command endpoint,
media mount, business credentials, or automatic CPU fallback.
"""
from __future__ import annotations

import argparse
import hmac
import pathlib
import signal
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .broker import Broker
from .model import Detector, ResourcePolicy
from .protocol import (MAX_BODY_BYTES, MAX_RESPONSE_BYTES, ProtocolError, Request,
                       canonical, decode_json, fields, token)


class Server(ThreadingHTTPServer):
    daemon_threads = True
    block_on_close = False
    request_queue_size = 8

    def __init__(self, address, broker, auth_token, max_handlers=8):
        if not 32 <= len(auth_token) <= 256 or not auth_token.isascii() or any(c.isspace() for c in auth_token):
            raise ValueError('token must contain 32–256 non-whitespace ASCII characters')
        if not 1 <= max_handlers <= 8:
            raise ValueError('handler count must be bounded')
        self.broker, self.auth_token = broker, auth_token
        self.slots = threading.BoundedSemaphore(max_handlers)
        super().__init__(address, Handler)

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            try:
                request.settimeout(1)
                request.sendall(b'HTTP/1.0 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\nRetry-After: 2\r\n\r\n')
            except OSError:
                pass
            finally:
                self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()

    def handle_error(self, request, client_address):
        pass  # no payload, token or request/path logging


class Handler(BaseHTTPRequestHandler):
    server_version = 'ImageInference/1'
    sys_version = ''

    def setup(self):
        super().setup()
        self.connection.settimeout(5)
        # An idle timeout alone permits a one-byte-per-four-seconds slow sender.
        # This timer bounds headers + authenticated body upload in total.
        self.upload_timer = threading.Timer(5, self._expire_upload)
        self.upload_timer.daemon = True
        self.upload_timer.start()

    def _expire_upload(self):
        try:
            self.connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass

    def finish(self):
        self.upload_timer.cancel()
        super().finish()

    def log_message(self, *args):
        pass

    def _send(self, status, value, retryable=False):
        data = canonical(value)
        if len(data) > MAX_RESPONSE_BYTES:
            raise ProtocolError('response_too_large', 503)
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Connection', 'close')
        if retryable:
            self.send_header('Retry-After', '2')
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True

    def _auth(self):
        values = self.headers.get_all('Authorization', [])
        supplied = values[0] if len(values) == 1 else ''
        if not hmac.compare_digest(supplied.encode(), ('Bearer ' + self.server.auth_token).encode()):
            raise ProtocolError('unauthorized', 401)

    def _body(self):
        if self.headers.get('Transfer-Encoding') or self.headers.get_all('Content-Length', []) == []:
            raise ProtocolError('content_length_required', 411)
        lengths = self.headers.get_all('Content-Length', [])
        if len(lengths) != 1 or not lengths[0].isascii() or not lengths[0].isdigit() or len(lengths[0]) > 8:
            raise ProtocolError('invalid_content_length')
        size = int(lengths[0])
        if not 0 < size <= MAX_BODY_BYTES:
            raise ProtocolError('request_too_large', 413)
        if self.headers.get('Content-Type') != 'application/json':
            raise ProtocolError('unsupported_content_type', 415)
        # Bound the whole authenticated upload, not only each individual recv.
        end = time.monotonic() + 5
        pieces, received = [], 0
        while received < size:
            remaining = end - time.monotonic()
            if remaining <= 0:
                raise ProtocolError('upload_timeout', 408, True)
            self.connection.settimeout(remaining)
            piece = self.rfile.read1(min(65536, size - received))
            if not piece:
                break
            pieces.append(piece)
            received += len(piece)
        self.connection.settimeout(5)
        data = b''.join(pieces)
        if len(data) != size:
            raise ProtocolError('incomplete_request')
        self.upload_timer.cancel()
        return decode_json(data)

    def _dispatch(self):
        self._auth()
        if self.command == 'GET':
            self.upload_timer.cancel()
        if self.command == 'GET' and self.path == '/health/live':
            return 200, {'live': True, 'service_instance': self.server.broker.instance}
        if self.command == 'GET' and self.path == '/health/ready':
            health = self.server.broker.health()
            return 200 if health['accepting'] else 503, health
        if self.command == 'POST' and self.path == '/v1/detect':
            request = Request.parse(self._body())
            job = self.server.broker.submit(request)
            # The deadline governs waiting, not cancellation of an already-running kernel.
            return 200, self.server.broker.wait(job)
        if self.command == 'POST' and self.path == '/v1/cancel':
            body = self._body()
            fields(body, 'request_id attempt_id')
            return 200, self.server.broker.cancel(token(body['request_id']), token(body['attempt_id']))
        raise ProtocolError('not_found', 404)

    def _handle(self):
        try:
            status, value = self._dispatch()
            self._send(status, value)
        except ProtocolError as exc:
            try:
                self._send(exc.status, {'error': {'code': exc.code, 'retryable': exc.retryable}}, exc.retryable)
            except (OSError, ProtocolError):
                pass
        except (OSError, socket.timeout):
            # Disconnection never publishes or frees a running GPU slot early.
            pass
        finally:
            self.close_connection = True

    do_GET = _handle
    do_POST = _handle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=8921)
    parser.add_argument('--model', required=True)
    parser.add_argument('--model-sha256', required=True)
    parser.add_argument('--token-file', required=True)
    parser.add_argument('--detector-size', type=int, choices=(160, 640), default=640)
    parser.add_argument('--arena-mib', type=int, choices=range(64, 257), default=128)
    parser.add_argument('--min-free-mib', type=int, default=600)
    parser.add_argument('--waiting', type=int, choices=(0, 1), default=1)
    args = parser.parse_args()
    if args.min_free_mib < 600:
        parser.error('resource floor below discovery limit is not validated')
    token_path = pathlib.Path(args.token_file)
    if token_path.stat().st_mode & 0o077:
        parser.error('token file must be private (mode 0600)')
    auth_token = token_path.read_text().strip()
    resources = ResourcePolicy(min_free_mib=args.min_free_mib)
    model, fault = None, None
    if resources.sample()['busy']:
        fault = 'startup_resource_busy'
    else:
        try:
            model = Detector(args.model, args.model_sha256, args.detector_size, args.arena_mib)
        except ProtocolError as exc:
            fault = exc.code
        except Exception:
            fault = 'model_initialization_failed'
    broker = Broker(model, resources, waiting=args.waiting)
    broker.fault = fault
    server = Server((args.host, args.port), broker, auth_token)
    def stop(*_):
        threading.Thread(target=server.shutdown, daemon=True).start()
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.serve_forever(poll_interval=.2)
    finally:
        server.server_close()
        broker.close()


if __name__ == '__main__':
    main()
