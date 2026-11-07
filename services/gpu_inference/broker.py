"""Single execution lane, bounded waiting queue, volatile computation deduplication."""
from __future__ import annotations

import collections
import queue
import threading
import time
import uuid
from dataclasses import dataclass, field

from .protocol import ProtocolError, Request, Result, SCHEMA, now_ms


@dataclass
class Job:
    request: Request | None
    request_hash: str
    attempt_id: str
    deadline_ms: int
    submitted: float = field(default_factory=time.monotonic)
    done: threading.Event = field(default_factory=threading.Event)
    cancelled: threading.Event = field(default_factory=threading.Event)
    result: dict | None = None
    error: ProtocolError | None = None
    finished: float | None = None


class Broker:
    def __init__(self, model, resources, waiting=1, cache_entries=64, cache_ttl=60):
        if waiting not in (0, 1) or not 1 <= cache_entries <= 128 or not 1 <= cache_ttl <= 120:
            raise ValueError('unverified concurrency or unbounded cache')
        self.model, self.resources = model, resources
        self.instance = str(uuid.uuid4())
        self.capacity = waiting + 1
        self.cache_entries, self.cache_ttl = cache_entries, cache_ttl
        self.jobs = collections.OrderedDict()
        self.lock = threading.RLock()
        self.queue = queue.Queue(maxsize=self.capacity)
        self.inflight = 0
        self.running = 0
        self.fault = None
        self.closed = False
        self.draining = False
        self.release = {'managed': False}
        self.thread = threading.Thread(target=self._run, name='gpu-single-lane', daemon=True)
        self.thread.start()

    def _prune(self):
        now = time.monotonic()
        for key, job in list(self.jobs.items()):
            if job.finished is not None and now - job.finished >= self.cache_ttl:
                del self.jobs[key]
        while len(self.jobs) >= self.cache_entries:
            key = next((key for key, job in self.jobs.items() if job.done.is_set()), None)
            if key is None:
                break
            del self.jobs[key]

    def health(self):
        resource = self.resources.sample()
        with self.lock:
            ready = not self.closed and self.fault is None and self.model is not None
            return {'live': True, 'model_ready': ready, 'service_instance': self.instance,
                    'execution': self.model.execution if self.model else None,
                    'running': self.running, 'queued': self.inflight - self.running,
                    'capacity': self.capacity, 'concurrency': 1, 'resources': resource,
                    'release': self.release, 'draining': self.draining,
                    'drained': self.draining and self.inflight == 0,
                    'busy': resource['busy'] or self.inflight >= self.capacity,
                    'accepting': ready and not self.draining and not resource['busy'] and self.inflight < self.capacity,
                    'fault': self.fault}

    def submit(self, request):
        with self.lock:
            self._prune()
            previous = self.jobs.get(request.request_id)
            if previous:
                if previous.request_hash != request.request_hash:
                    raise ProtocolError('idempotency_conflict', 409)
                if not previous.done.is_set():
                    raise ProtocolError('request_in_progress', 409, True)
                return previous
            if self.draining:
                raise ProtocolError('service_draining', 503, True)
            if self.closed or self.model is None or self.fault:
                raise ProtocolError('model_not_ready', 503, True)
            if request.model_sha256 != self.model.model_sha256 or request.preprocess_version != self.model.preprocess_version:
                raise ProtocolError('model_version_mismatch', 409)
            if self.inflight >= self.capacity or len(self.jobs) >= self.cache_entries:
                raise ProtocolError('queue_full', 429, True)
            if self.resources.sample()['busy']:
                raise ProtocolError('resource_busy', 503, True)
            job = Job(request, request.request_hash, request.binding.attempt_id, request.deadline_ms)
            self.jobs[request.request_id] = job
            self.inflight += 1
            self.queue.put_nowait(job)
            return job

    def begin_drain(self):
        with self.lock:
            self.draining = True
            return {'draining': True, 'drained': self.inflight == 0,
                    'running': self.running, 'queued': self.inflight - self.running}

    def cancel(self, request_id, attempt_id):
        with self.lock:
            job = self.jobs.get(request_id)
            if not job:
                raise ProtocolError('request_not_found', 404)
            if job.attempt_id != attempt_id:
                raise ProtocolError('attempt_mismatch', 409)
            job.cancelled.set()
            # Do not claim ORT has stopped. The single execution slot stays reserved
            # until the worker actually exits infer(). Never return an old cached result.
            return {'cancel_requested': True, 'execution_finished': job.done.is_set()}

    def wait(self, job):
        remaining = max(0, (job.deadline_ms - now_ms()) / 1000)
        if not job.done.wait(remaining):
            job.cancelled.set()
            raise ProtocolError('deadline_exceeded', 504, True)
        if job.cancelled.is_set():
            raise ProtocolError('cancelled', 409)
        if job.error:
            raise job.error
        if now_ms() >= job.deadline_ms:
            raise ProtocolError('deadline_exceeded', 504, True)
        return job.result

    def _run(self):
        while True:
            try:
                job = self.queue.get(timeout=.2)
            except queue.Empty:
                if self.closed:
                    return
                continue
            request = job.request
            with self.lock:
                self.running = 1
            started = time.monotonic()
            try:
                if job.cancelled.is_set() or now_ms() >= job.deadline_ms or self.closed:
                    raise ProtocolError('cancelled_or_expired', 409)
                if self.fault:
                    raise ProtocolError('model_not_ready', 503, True)
                if self.resources.sample()['busy']:
                    raise ProtocolError('resource_busy', 503, True)
                faces = self.model.infer(request)
                elapsed = (time.monotonic() - started) * 1000
                if job.cancelled.is_set() or now_ms() >= job.deadline_ms or self.closed:
                    raise ProtocolError('cancelled_or_expired', 409)
                response = {'schema_version': SCHEMA, 'context': request.context(),
                            'request_hash': request.request_hash, 'service_instance': self.instance,
                            'execution': self.model.execution, 'faces': faces,
                            'timing': {'queue_ms': (started-job.submitted)*1000, 'inference_ms': elapsed,
                                       'total_ms': (time.monotonic()-job.submitted)*1000}}
                Result.parse(response, request)
                job.result = response
            except ProtocolError as exc:
                job.error = exc
            except Exception:
                # CUDA errors/OOM can poison a session. Fail closed until an operator
                # replaces this isolated process; never silently change provider.
                job.error = ProtocolError('inference_failed', 503, True)
                with self.lock:
                    self.fault = 'inference_failed'
            finally:
                with self.lock:
                    job.request = None  # do not retain encoded images in the replay cache
                    job.finished = time.monotonic()
                    self.inflight -= 1
                    self.running = 0
                    job.done.set()
                self.queue.task_done()

    def close(self):
        with self.lock:
            if self.closed:
                return
            self.closed = True
            for job in self.jobs.values():
                if not job.done.is_set():
                    job.cancelled.set()
        # This is a drain signal, not a forced model cancellation. Do not overlap a
        # replacement instance until the old process has actually exited.
        self.thread.join(timeout=5)
