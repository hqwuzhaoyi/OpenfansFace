"""Typed VM transport. No automatic retry, CPU fallback, file or business writes."""
from __future__ import annotations

import ipaddress
import urllib.error
import urllib.parse
import urllib.request

from .protocol import (MAX_BODY_BYTES, MAX_RESPONSE_BYTES, ProtocolError, Request,
                       Result, canonical, decode_json, now_ms, token)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ProtocolError('redirect_refused', 502)


class Client:
    def __init__(self, base_url: str, auth_token: str, timeout: float = 30, allow_private_http=False):
        url = urllib.parse.urlsplit(base_url)
        if url.scheme not in ('http', 'https') or not url.hostname or url.username or url.password or url.query or url.fragment or url.path not in ('', '/'):
            raise ValueError('invalid inference origin')
        if url.scheme == 'http':
            try:
                address = ipaddress.ip_address(url.hostname)
                allowed = address.is_loopback or (allow_private_http and address.is_private)
            except ValueError:
                allowed = url.hostname == 'localhost'
            if not allowed:
                raise ValueError('remote HTTP requires an explicitly trusted private IP; prefer TLS or a private tunnel')
        if not 0 < timeout <= 120 or not 32 <= len(auth_token) <= 256 or any(c.isspace() for c in auth_token) or not auth_token.isascii():
            raise ValueError('invalid token or timeout')
        self.base_url, self.auth_token, self.timeout = base_url.rstrip('/'), auth_token, timeout
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def _call(self, path, body=None, timeout=None):
        data = canonical(body) if body is not None else None
        if data is not None and len(data) > MAX_BODY_BYTES:
            raise ProtocolError('request_too_large', 413)
        headers = {'Authorization': 'Bearer ' + self.auth_token, 'Content-Type': 'application/json'}
        request = urllib.request.Request(self.base_url + path, data=data, headers=headers)
        try:
            with self.opener.open(request, timeout=timeout or self.timeout) as response:
                payload = response.read(MAX_RESPONSE_BYTES + 1)
                if len(payload) > MAX_RESPONSE_BYTES:
                    raise ProtocolError('response_too_large', 502)
                if response.headers.get_content_type() != 'application/json':
                    raise ProtocolError('invalid_response_type', 502)
                return decode_json(payload)
        except urllib.error.HTTPError as exc:
            payload = exc.read(MAX_RESPONSE_BYTES + 1)
            try:
                value = decode_json(payload)
                if path == '/health/ready' and exc.code == 503 and type(value.get('model_ready')) is bool and type(value.get('accepting')) is bool:
                    return value
                error = value.get('error', {})
                code = token(error.get('code'))
                retryable = error.get('retryable') is True
            except (ProtocolError, AttributeError):
                code, retryable = 'remote_unavailable', exc.code in (429, 503, 504)
            raise ProtocolError(code, exc.code, retryable) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            # Timeout leaves remote execution unknown. Queue owner chooses a new
            # attempt only after fencing the old one; this client never retries.
            raise ProtocolError('transport_unavailable', 503, True) from None

    def detect(self, request: Request) -> Result:
        # Validate locally even if callers instantiated dataclasses directly.
        request = Request.parse(request.wire())
        remaining = (min(request.deadline_ms, request.binding.lease_expires_at_ms) - now_ms()) / 1000
        if remaining <= 0:
            raise ProtocolError('expired_or_invalid_deadline', 408)
        return Result.parse(self._call('/v1/detect', request.wire(), min(self.timeout, remaining)), request)

    def cancel(self, request_id: str, attempt_id: str) -> dict:
        return self._call('/v1/cancel', {'request_id': token(request_id), 'attempt_id': token(attempt_id)})

    def ready(self) -> dict:
        return self._call('/health/ready')
