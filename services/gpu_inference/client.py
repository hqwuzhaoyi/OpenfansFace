"""Typed VM transport with bounded same-intent retry; no fallback or business writes."""
from __future__ import annotations

import email.utils
import ipaddress
import http.client
import random
import urllib.error
import urllib.parse
import urllib.request

from .protocol import (MAX_BODY_BYTES, MAX_RESPONSE_BYTES, ProtocolError, Request,
                       Result, canonical, decode_json, token, Clock,
                       CLIENT_MAX_SENDS, CLIENT_BACKOFF_SECONDS, CLIENT_BACKOFF_CAP_SECONDS,
                       CLIENT_JITTER_MIN, CLIENT_JITTER_MAX, CLIENT_RETRY_AFTER_CAP_SECONDS)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ProtocolError('redirect_refused', 502)


class Client:
    def __init__(self, base_url: str, auth_token: str, timeout: float = 30, allow_private_http=False,
                 *, clock=None, jitter=None, on_wait=None):
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
        self.clock, self.jitter, self.on_wait = clock or Clock(), jitter or random.uniform, on_wait
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def _read_response(self, response):
        # read(limit) can return EOF-short bytes without IncompleteRead. Only
        # framing-proven truncation is transport uncertainty; complete invalid
        # JSON/context remains a nonretryable protocol error.
        lengths = response.headers.get_all('Content-Length', [])
        if len(lengths) > 1 or (lengths and response.headers.get('Transfer-Encoding')):
            raise ProtocolError('invalid_response_framing', 502)
        expected = None
        if lengths:
            length = lengths[0]
            if not length.isascii() or not length.isdigit():
                raise ProtocolError('invalid_response_framing', 502)
            if len(length) > 10 or int(length) > MAX_RESPONSE_BYTES:
                raise ProtocolError('response_too_large', 502)
            expected = int(length)
        try:
            payload = response.read(MAX_RESPONSE_BYTES + 1)
        except (http.client.IncompleteRead, OSError):
            raise ProtocolError('transport_unavailable', 503, True) from None
        if len(payload) > MAX_RESPONSE_BYTES:
            raise ProtocolError('response_too_large', 502)
        if expected is not None and len(payload) < expected:
            raise ProtocolError('transport_unavailable', 503, True)
        return payload

    def _retry_after(self, response):
        # Headers remain authoritative when the response body is truncated. Parse
        # this before reading the body so transport recovery cannot bypass a known
        # minimum delay or the bounded-policy stop for a hint above ten seconds.
        retry_after, allowed = None, True
        if response.code in (429, 503):
            header = response.headers.get('Retry-After', '')
            try:
                seconds = int(header) if header.isascii() and header.isdigit() else (
                    email.utils.parsedate_to_datetime(header).timestamp() - self.clock.now_ms() / 1000)
                if seconds > CLIENT_RETRY_AFTER_CAP_SECONDS:
                    allowed = False
                else:
                    retry_after = max(0, seconds)
            except (ValueError, TypeError, OverflowError):
                pass
        return retry_after, allowed

    def _call(self, path, body=None, timeout=None):
        data = canonical(body) if body is not None else None
        if data is not None and len(data) > MAX_BODY_BYTES:
            raise ProtocolError('request_too_large', 413)
        headers = {'Authorization': 'Bearer ' + self.auth_token, 'Content-Type': 'application/json'}
        request = urllib.request.Request(self.base_url + path, data=data, headers=headers)
        try:
            with self.opener.open(request, timeout=timeout or self.timeout) as response:
                payload = self._read_response(response)
                if response.headers.get_content_type() != 'application/json':
                    raise ProtocolError('invalid_response_type', 502)
                return decode_json(payload)
        except urllib.error.HTTPError as exc:
            retry_after, header_allows_retry = self._retry_after(exc)
            try:
                payload = self._read_response(exc)
            except ProtocolError as error:
                if error.code != 'transport_unavailable':
                    raise
                # Only known transient statuses can recover an incomplete error
                # body. In particular, body loss cannot turn 401/403 into retry.
                transient = exc.code in (429, 503)
                raise ProtocolError('transport_unavailable' if transient else 'remote_unavailable',
                                    exc.code, transient and header_allows_retry,
                                    retry_after=retry_after) from None
            finally:
                exc.close()
            try:
                value = decode_json(payload)
                if path == '/health/ready' and exc.code == 503 and type(value.get('model_ready')) is bool and type(value.get('accepting')) is bool:
                    return value
                error = value.get('error', {})
                code = token(error.get('code'))
                retryable = error.get('retryable') is True
            except (ProtocolError, AttributeError):
                code, retryable = 'remote_unavailable', False
            retryable = retryable and header_allows_retry and exc.code not in (401, 403)
            raise ProtocolError(code, exc.code, retryable, retry_after=retry_after) from None
        except (urllib.error.URLError, http.client.IncompleteRead, TimeoutError, OSError):
            # Execution is unknown: detect may only replay the SAME frozen request.
            # This never authorizes a new frame/attempt or a business-state write.
            raise ProtocolError('transport_unavailable', 503, True) from None

    def detect(self, request: Request, *, cancelled=None) -> Result:
        # Freeze once. All sends use the same ID, image bytes, hash, attempt,
        # deadline and lease; retries do not extend any budget.
        request = Request.parse(request.wire(), at_ms=self.clock.now_ms())
        body = request.wire()
        deadline = min(request.deadline_ms, request.binding.lease_expires_at_ms)
        expires = self.clock.monotonic() + (deadline - self.clock.now_ms()) / 1000
        retry_codes = {'queue_full', 'resource_busy', 'request_in_progress', 'transport_unavailable'}
        for send in range(CLIENT_MAX_SENDS):
            if cancelled is not None and cancelled.is_set():
                raise ProtocolError('cancelled', 409)
            remaining = min(expires - self.clock.monotonic(), (deadline - self.clock.now_ms()) / 1000)
            if remaining <= 0:
                raise ProtocolError('expired_or_invalid_deadline', 408)
            try:
                value = self._call('/v1/detect', body, min(self.timeout, remaining))
                if cancelled is not None and cancelled.is_set():
                    raise ProtocolError('cancelled', 409)
                if self.clock.monotonic() >= expires or self.clock.now_ms() >= deadline:
                    raise ProtocolError('expired_or_invalid_deadline', 408)
                return Result.parse(value, request)
            except ProtocolError as exc:
                if cancelled is not None and cancelled.is_set():
                    raise ProtocolError('cancelled', 409) from None
                if not exc.retryable or exc.code not in retry_codes or send + 1 == CLIENT_MAX_SENDS:
                    raise
                remaining = min(expires - self.clock.monotonic(), (deadline - self.clock.now_ms()) / 1000)
                delay = min(CLIENT_BACKOFF_CAP_SECONDS, CLIENT_BACKOFF_SECONDS * 2 ** send)
                delay *= self.jitter(CLIENT_JITTER_MIN, CLIENT_JITTER_MAX)
                delay = max(delay, exc.retry_after or 0)
                if remaining <= delay:
                    raise
                if self.on_wait:
                    self.on_wait(exc.code, send + 1, delay)
                if cancelled is None:
                    self.clock.sleep(delay)
                elif self.clock.wait(cancelled, delay):
                    raise ProtocolError('cancelled', 409)
        raise AssertionError('bounded retry loop exhausted without outcome')

    def cancel(self, request_id: str, attempt_id: str) -> dict:
        return self._call('/v1/cancel', {'request_id': token(request_id), 'attempt_id': token(attempt_id)})

    def drain(self) -> dict:
        return self._call('/v1/drain', {})

    def ready(self) -> dict:
        return self._call('/health/ready')
