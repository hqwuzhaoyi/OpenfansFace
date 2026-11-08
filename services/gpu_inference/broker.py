"""Single execution lane, bounded resource waiting and volatile deduplication."""
from __future__ import annotations

import collections
import logging
import threading
import uuid
from dataclasses import dataclass, field

from .protocol import (Clock, ProtocolError, Request, Result, SCHEMA,
                       RESOURCE_WAIT_SECONDS, RESOURCE_POLL_SECONDS, RESOURCE_REARMS,
                       retry_policy)

LOG = logging.getLogger(__name__)


@dataclass(eq=False)
class Job:
    request: Request | None
    request_hash: str
    attempt_id: str
    deadline_ms: int
    submitted: float
    expires: float
    resource_expires: float
    rearms: int = 0
    execution_started: bool = False
    waiting_reason: str | None = None
    done: threading.Event = field(default_factory=threading.Event)
    cancelled: threading.Event = field(default_factory=threading.Event)
    wake: threading.Event = field(default_factory=threading.Event)
    result: dict | None = None
    error: ProtocolError | None = None
    finished: float | None = None


class Broker:
    def __init__(self, model, resources, waiting=1, cache_entries=64, cache_ttl=60, *, clock=None):
        if waiting not in (0, 1) or not 1 <= cache_entries <= 128 or not 1 <= cache_ttl <= 120:
            raise ValueError('unverified concurrency or unbounded cache')
        self.model, self.resources = model, resources
        self.clock = clock or Clock()
        self.instance = str(uuid.uuid4())
        self.capacity = waiting + 1
        self.cache_entries, self.cache_ttl = cache_entries, cache_ttl
        self.jobs = collections.OrderedDict()
        self.lock = threading.RLock()
        self.changed = threading.Condition(self.lock)
        self.queue = collections.deque()
        self.inflight = 0
        self.running = 0
        self.resource_waiting = 0
        self.waiting_reason = None
        self.waiting_job = None
        self.fault = None
        self.closed = False
        self.draining = False
        self.release = {'managed': False}
        self.thread = threading.Thread(target=self._run, name='gpu-single-lane', daemon=True)
        self.thread.start()
        self.expirer = threading.Thread(target=self._expire_waiting, name='gpu-queue-deadlines', daemon=True)
        self.expirer.start()

    def _prune(self):
        now = self.clock.monotonic()
        wall_ms = self.clock.now_ms()
        # Never evict a replayable identity before its original deadline. A lost
        # response must not turn cache pressure/TTL expiry into a second execution.
        for key, job in list(self.jobs.items()):
            if (job.finished is not None and now >= job.expires and wall_ms >= job.deadline_ms
                    and now - job.finished >= self.cache_ttl):
                del self.jobs[key]
        while len(self.jobs) >= self.cache_entries:
            key = next((key for key, job in self.jobs.items()
                        if job.done.is_set() and now >= job.expires and wall_ms >= job.deadline_ms), None)
            if key is None:
                break
            del self.jobs[key]

    def health(self):
        resource = self.resources.sample()
        with self.lock:
            self._prune()
            ready = not self.closed and self.fault is None and self.model is not None
            resource_ready = resource.get('available') is True and resource.get('busy') is False
            return {'live': True, 'model_ready': ready, 'service_instance': self.instance,
                    'execution': self.model.execution if self.model else None,
                    'running': self.running, 'queued': self.inflight - self.running,
                    'resource_waiting': self.resource_waiting, 'waiting_reason': self.waiting_reason,
                    'capacity': self.capacity, 'concurrency': 1, 'resources': resource,
                    'retry_policy': retry_policy(), 'resource_ready': resource_ready,
                    'release': self.release, 'draining': self.draining,
                    'drained': self.draining and self.inflight == 0,
                    'busy': not resource_ready or self.inflight >= self.capacity,
                    'accepting': ready and not self.draining and self.inflight < self.capacity
                                 and len(self.jobs) < self.cache_entries,
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
                # Only a proven pre-execution resource refusal is rearmable. All
                # success, cancellation, fault and uncertain executions stay sealed.
                if not (previous.error and previous.error.code == 'resource_busy'
                        and not previous.execution_started and not previous.cancelled.is_set()
                        and previous.rearms < RESOURCE_REARMS):
                    return previous
            if self.draining:
                raise ProtocolError('service_draining', 503, True)
            if self.closed or self.model is None or self.fault:
                raise ProtocolError('model_not_ready', 503, True)
            if request.model_sha256 != self.model.model_sha256 or request.preprocess_version != self.model.preprocess_version:
                raise ProtocolError('model_version_mismatch', 409)
            deadline_ms = min(request.deadline_ms, request.binding.lease_expires_at_ms)
            remaining = (deadline_ms - self.clock.now_ms()) / 1000
            now = self.clock.monotonic()
            if remaining <= 0 or (previous and now >= previous.expires):
                raise ProtocolError('expired_or_invalid_deadline', 408)
            if self.inflight >= self.capacity or (not previous and len(self.jobs) >= self.cache_entries):
                LOG.info('gpu_refusal hash=%s reason=queue_full', request.request_hash[:16])
                raise ProtocolError('queue_full', 429, True)
            expires = previous.expires if previous else now + remaining
            job = Job(request, request.request_hash, request.binding.attempt_id, deadline_ms,
                      previous.submitted if previous else now, expires,
                      min(now + RESOURCE_WAIT_SECONDS, expires),
                      rearms=previous.rearms + 1 if previous else 0,
                      waiting_reason='execution_lane_busy' if self.inflight else None)
            self.jobs[request.request_id] = job
            self.inflight += 1
            self.queue.append(job)
            self.changed.notify_all()
            return job

    def begin_drain(self):
        with self.lock:
            self.draining = True
            # Already owned work retains its finite budget; no new admission or
            # resource rearm is allowed. A running kernel still owns its slot.
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
            job.wake.set()
            if not job.execution_started:
                self._finish(job, error=ProtocolError('cancelled', 409))
            return {'cancel_requested': True, 'execution_finished': job.done.is_set()}

    def _finish(self, job, *, result=None, error=None):
        """Called under the ownership lock; every admission is released once.

        The expirer/canceller may finish only never-started work. The model lane
        alone finishes execution_started jobs, after infer has actually returned.
        Removed deque entries leave no accumulating tombstones behind a kernel.
        """
        if job.done.is_set():
            return
        job.result, job.error = result, error
        job.request = None
        job.finished = self.clock.monotonic()
        self.inflight -= 1
        if job.execution_started:
            self.running = 0
        try:
            self.queue.remove(job)
        except ValueError:
            pass  # the lane already owns this head
        if self.waiting_job is job:
            self.waiting_job = None
            self.resource_waiting, self.waiting_reason = 0, None
        if error:
            LOG.info('gpu_request_end hash=%s reason=%s executed=%s waiting_reason=%s',
                     job.request_hash[:16], error.code, job.execution_started, job.waiting_reason)
        job.done.set()
        job.wake.set()
        self.changed.notify_all()

    def _expire_waiting(self):
        # One independent timer, not a thread/timer per request. It never touches
        # running kernel ownership. Admission and the infer transition use this
        # same lock, so a timed-out/cancelled admission cannot begin execution.
        with self.changed:
            while not self.closed:
                delay = RESOURCE_POLL_SECONDS
                pending = False
                for job in list(self.jobs.values()):
                    if job.done.is_set() or job.execution_started:
                        continue
                    pending = True
                    try:
                        self._check_current(job)
                        self._check_resource_budget(job)
                    except ProtocolError as error:
                        self._finish(job, error=error)
                        continue
                    delay = min(delay, self._remaining(job),
                                job.resource_expires - self.clock.monotonic())
                self.changed.wait(max(.001, delay) if pending else None)

    def _remaining(self, job):
        return min(job.expires - self.clock.monotonic(), (job.deadline_ms - self.clock.now_ms()) / 1000)

    def wait(self, job):
        if not self.clock.wait(job.done, max(0, self._remaining(job))):
            with self.lock:
                if not job.done.is_set():
                    job.cancelled.set()
                    job.wake.set()
                    if not job.execution_started:
                        self._finish(job, error=ProtocolError('deadline_exceeded', 504, True))
            raise ProtocolError('deadline_exceeded', 504, True)
        if job.cancelled.is_set():
            raise ProtocolError('cancelled', 409)
        if job.error:
            raise job.error
        if self._remaining(job) <= 0:
            raise ProtocolError('deadline_exceeded', 504, True)
        return job.result

    def _check_current(self, job):
        if job.done.is_set():
            raise job.error or ProtocolError('cancelled', 409)
        if job.cancelled.is_set() or self.closed:
            raise ProtocolError('cancelled', 409)
        if self._remaining(job) <= 0:
            raise ProtocolError('deadline_exceeded', 504, True)
        if self.fault:
            raise ProtocolError('model_not_ready', 503, True)

    def _check_resource_budget(self, job):
        if self.clock.monotonic() >= job.resource_expires:
            code = 'resource_busy' if job.rearms < RESOURCE_REARMS else 'resource_wait_exhausted'
            raise ProtocolError(code, 503, code == 'resource_busy')

    def _await_resources(self, job):
        waiting_started = False
        outcome = 'stopped'
        try:
            while True:
                self._check_current(job)
                self._check_resource_budget(job)
                resource = self.resources.sample(timeout=min(3, self._remaining(job),
                                                             job.resource_expires - self.clock.monotonic()))
                # Sampling may itself block; recheck deadline/cancellation afterwards.
                self._check_current(job)
                self._check_resource_budget(job)
                if resource.get('available') is True and resource.get('busy') is False:
                    outcome = 'ready'
                    return
                reason = resource.get('reason')
                if reason not in ('telemetry_unavailable', 'memory_low', 'utilization_high'):
                    reason = 'telemetry_unavailable' if resource.get('available') is not True else 'resource_busy'
                with self.lock:
                    self._check_current(job)
                    self.waiting_job = job
                    self.resource_waiting, self.waiting_reason = 1, reason
                    job.waiting_reason = reason
                if not waiting_started:
                    LOG.info('gpu_wait_start hash=%s reason=%s rearm=%s available=%s free_mib=%s utilization_pct=%s',
                             job.request_hash[:16], reason, job.rearms, resource.get('available') is True,
                             resource.get('memory_free_mib'), resource.get('utilization_pct'))
                    waiting_started = True
                remaining = min(self._remaining(job), job.resource_expires - self.clock.monotonic())
                if remaining <= 0:
                    code = 'resource_busy' if job.rearms < RESOURCE_REARMS else 'resource_wait_exhausted'
                    raise ProtocolError(code, 503, code == 'resource_busy')
                self.clock.resource_wait(job.wake, min(RESOURCE_POLL_SECONDS, remaining))
        finally:
            if waiting_started:
                LOG.info('gpu_wait_end hash=%s reason=%s outcome=%s queue_ms=%.1f', job.request_hash[:16],
                         job.waiting_reason, outcome, (self.clock.monotonic()-job.submitted)*1000)
            with self.lock:
                if self.waiting_job is job:
                    self.waiting_job = None
                    self.resource_waiting, self.waiting_reason = 0, None

    def _run(self):
        while True:
            with self.changed:
                while not self.queue and not self.closed:
                    self.changed.wait()
                if self.closed:
                    return
                job = self.queue.popleft()
            result, error = None, None
            try:
                self._await_resources(job)
                with self.lock:
                    self._check_current(job)
                    self._check_resource_budget(job)
                    self.running = 1
                    job.execution_started = True
                    request = job.request
                started = self.clock.monotonic()
                faces = self.model.infer(request)
                elapsed = (self.clock.monotonic() - started) * 1000
                self._check_current(job)
                response = {'schema_version': SCHEMA, 'context': request.context(),
                            'request_hash': request.request_hash, 'service_instance': self.instance,
                            'execution': self.model.execution, 'faces': faces,
                            'timing': {'queue_ms': (started-job.submitted)*1000, 'inference_ms': elapsed,
                                       'total_ms': (self.clock.monotonic()-job.submitted)*1000}}
                Result.parse(response, request)
                result = response
            except ProtocolError as exc:
                error = exc
            except Exception:
                error = ProtocolError('inference_failed', 503, True)
                with self.lock:
                    if not job.done.is_set():
                        self.fault = 'inference_failed'
            finally:
                with self.lock:
                    # If the timer already finished a never-started admission,
                    # preserve that immutable receipt and its released counter.
                    self._finish(job, result=result, error=error)
                request = None

    def close(self):
        with self.lock:
            if self.closed:
                return
            self.closed = True
            for job in self.jobs.values():
                if not job.done.is_set():
                    job.cancelled.set()
                    job.wake.set()
                    if not job.execution_started:
                        self._finish(job, error=ProtocolError('cancelled', 409))
            self.changed.notify_all()
        self.expirer.join(timeout=1)
        # Never overlap replacement until the old process actually exits infer().
        self.thread.join(timeout=5)
