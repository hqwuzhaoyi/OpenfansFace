"""Bounded synthetic stage probe; no media, identity matching or business writes.

Uses an existing environment/model only. CPU measurements are on this same host,
not a claim about the production VM. At most one CUDA session/inference at a time.
"""
from __future__ import annotations

import argparse
import hashlib
import inspect
import json
import resource
import secrets
import threading
import time
import urllib.request
import uuid

from .broker import Broker
from .client import Client
from .model import Detector, ResourcePolicy
from .protocol import Binding, Fingerprint, ProtocolError, Request, now_ms
from .server import Server


def neighbors(ports):
    result = {}
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for port in ports:
        started = time.monotonic()
        try:
            with opener.open(f'http://127.0.0.1:{port}/health', timeout=2) as response:
                status = response.status
        except Exception:
            status = 'unavailable'
        result[str(port)] = {'status': status, 'latency_ms': round((time.monotonic()-started)*1000, 3)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--model', required=True)
    parser.add_argument('--model-sha256', required=True)
    parser.add_argument('--size', type=int, choices=(160, 640), default=640)
    parser.add_argument('--cpu-only', action='store_true')
    parser.add_argument('--neighbor-port', type=int, action='append', required=True,
                        help='Existing loopback /health listener; repeat for every coexisting service')
    args = parser.parse_args()
    if any(not 1 <= port <= 65535 for port in args.neighbor_port):
        parser.error('invalid neighbor port')
    policy = ResourcePolicy()
    before = policy.sample()
    neighbor_before = neighbors(args.neighbor_port)
    summary = {'scope': 'anonymous synthetic detector only, CPU baseline on same WSL host; no VM benchmark',
               'concurrency': 1, 'size': args.size, 'before': before, 'neighbors_before': neighbor_before}
    if (before['busy'] and not args.cpu_only) or any(x['status'] != 200 for x in neighbor_before.values()):
        summary.update(status='skipped', reason='resource_or_neighbor_preflight')
        print(json.dumps(summary)); return 3
    samples, neighbor_samples = [], []
    stop = threading.Event()
    def monitor():
        while not stop.wait(.3):
            samples.append(policy.sample())
            if len(samples) % 3 == 0:
                neighbor_samples.append(neighbors(args.neighbor_port))
    monitor_thread = threading.Thread(target=monitor, daemon=True)
    monitor_thread.start()
    broker = server = thread = None
    started = time.monotonic()
    usage_before = resource.getrusage(resource.RUSAGE_SELF)
    try:
        import cv2
        import numpy as np
        import onnxruntime as ort
        from insightface.model_zoo.scrfd import SCRFD
        cv2.setNumThreads(1)
        if args.cpu_only:
            # An independent baseline is still useful when GPU admission refuses work.
            # This is deliberately not labelled a completed CPU/GPU comparison.
            import pathlib
            if hashlib.sha256(pathlib.Path(args.model).read_bytes()).hexdigest() != args.model_sha256:
                raise ProtocolError('model_hash_mismatch', 503)
            options = ort.SessionOptions()
            options.intra_op_num_threads = options.inter_op_num_threads = 1
            session = ort.InferenceSession(args.model, sess_options=options, providers=['CPUExecutionProvider'])
            kwargs = {'model_file': args.model, 'session': session}
            if 'static_shape_sessions' in inspect.signature(SCRFD).parameters:
                kwargs['static_shape_sessions'] = False
            cpu = SCRFD(**kwargs)
            cpu.prepare(ctx_id=0, input_size=(args.size,args.size), det_thresh=.5, nms_thresh=.4)
            frames = [np.zeros((args.size,args.size,3),np.uint8),
                      np.broadcast_to(np.linspace(0,255,args.size,dtype=np.uint8)[None,:,None],(args.size,args.size,3)).copy(),
                      np.random.default_rng(19).integers(0,256,(args.size,args.size,3),dtype=np.uint8)]
            rows = []
            for iteration in range(2):
                for index, frame in enumerate(frames):
                    t = time.monotonic(); boxes, _ = cpu.detect(frame)
                    rows.append({'iteration':iteration, 'fixture':index,
                                 'cpu_detector_ms':round((time.monotonic()-t)*1000,3), 'face_count':len(boxes)})
            summary.update(status='cpu_baseline_only', samples=rows, providers=session.get_providers(),
                           model_sha256=args.model_sha256,
                           preprocess_version=f'scrfd-{args.size}-bgr8-det0.5-nms0.4-v1',
                           limitations=['WSL CPU, not production VM', 'No positive-face fixtures or full pipeline',
                                        'Not a completed CPU/GPU comparison'])
            return
        model = Detector(args.model, args.model_sha256, size=args.size, arena_mib=128)
        summary['execution'] = model.execution
        summary['gpu_model_load_and_readiness_ms'] = round((time.monotonic()-started)*1000, 3)
        summary['after_model'] = policy.sample()
        if summary['after_model']['busy']:
            raise ProtocolError('post_load_resource_busy', 503)
        options = ort.SessionOptions()
        options.intra_op_num_threads = options.inter_op_num_threads = 1
        cpu_session = ort.InferenceSession(args.model, sess_options=options, providers=['CPUExecutionProvider'])
        kwargs = {'model_file': args.model, 'session': cpu_session}
        if 'static_shape_sessions' in inspect.signature(SCRFD).parameters:
            kwargs['static_shape_sessions'] = False
        cpu = SCRFD(**kwargs)
        cpu.prepare(ctx_id=0, input_size=(args.size,args.size),det_thresh=.5,nms_thresh=.4)
        # Equal raster/preprocessing/configuration for both providers, no real-person data.
        frames = [np.zeros((args.size,args.size,3),np.uint8),
                  np.broadcast_to(np.linspace(0,255,args.size,dtype=np.uint8)[None,:,None],(args.size,args.size,3)).copy(),
                  np.random.default_rng(19).integers(0,256,(args.size,args.size,3),dtype=np.uint8)]
        wire_frames = []
        for index, frame in enumerate(frames):
            ok, encoded = cv2.imencode('.png',frame)
            if not ok: raise RuntimeError('fixture_encode_failed')
            data=encoded.tobytes()
            wire_frames.append(Request(str(uuid.uuid4()),Binding('isolated-task','isolated-video','isolated-batch',
                 'isolated-attempt','isolated-lease','isolated-generation',now_ms()+90_000,
                 Fingerprint(len(data),'0',hashlib.sha256(data).hexdigest()),'a'*64,'synthetic-detector-v1'),
                 args.model_sha256,model.preprocess_version,now_ms()+60_000,index,index*1000,
                 hashlib.sha256(data).hexdigest(),args.size,args.size,data))
        broker=Broker(model,policy,waiting=0)
        server=Server(('127.0.0.1',0),broker,secrets.token_urlsafe(32))
        thread=threading.Thread(target=server.serve_forever,kwargs={'poll_interval':.01},daemon=True);thread.start()
        client=Client(f'http://127.0.0.1:{server.server_address[1]}',server.auth_token)
        rows=[]
        for index,(frame,req) in enumerate(zip(frames,wire_frames)):
            if policy.sample()['busy']:raise ProtocolError('resource_busy',503)
            t=time.monotonic();cpu_boxes,cpu_points=cpu.detect(frame)
            cpu_ms=(time.monotonic()-t)*1000
            t=time.monotonic();result=client.detect(req);http_ms=(time.monotonic()-t)*1000
            boxes=np.asarray([f['bbox']+[f['score']] for f in result.faces],dtype=np.float32).reshape(-1,5)
            points=np.asarray([f['keypoints'] for f in result.faces],dtype=np.float32).reshape(-1,5,2)
            agreement=boxes.shape==cpu_boxes.shape and bool(np.allclose(boxes,cpu_boxes,rtol=1e-4,atol=1e-3))
            if len(boxes):agreement=agreement and cpu_points is not None and bool(np.allclose(points,cpu_points,rtol=1e-4,atol=1e-3))
            rows.append({'fixture':index,'image_bytes':len(req.image),'cpu_detector_ms':round(cpu_ms,3),
                         'gpu_http_ms':round(http_ms,3),'gpu_timing':result.timing,'face_count':len(boxes),
                         'detector_output_agrees':agreement})
        summary.update(status='completed',samples=rows,limitations=[
            'No positive-face fixtures or recognition/candidate consistency proof',
            'CPU baseline is same WSL host, not production VM',
            'Loopback transfer only, not VM network path',
            'Three samples with profiling/readiness warmup; not sustained throughput',
            'No higher concurrency or other GPU stages validated'])
        # Exercise the actual decoder boundary after successful model requests.
        bad=wire_frames[0]
        from dataclasses import replace
        try: model.infer(replace(bad,width=bad.width-1))
        except ProtocolError as exc:summary['invalid_dimensions_rejected']=exc.code
        else:raise RuntimeError('invalid_dimensions_accepted')
    except Exception as exc:
        summary.update(status='failed',error=exc.code if isinstance(exc,ProtocolError) else type(exc).__name__)
    finally:
        if server:server.shutdown();server.server_close()
        if broker:broker.close()
        if thread:thread.join(1)
        stop.set();monitor_thread.join(3)
        usage=resource.getrusage(resource.RUSAGE_SELF)
        summary.update(wall_ms=round((time.monotonic()-started)*1000,3),
                       process_user_cpu_s=usage.ru_utime-usage_before.ru_utime,
                       process_system_cpu_s=usage.ru_stime-usage_before.ru_stime,
                       process_peak_rss_kib=usage.ru_maxrss,
                       resource_samples=samples,neighbor_samples=neighbor_samples,
                       after=policy.sample(),neighbors_after=neighbors(args.neighbor_port))
        print(json.dumps(summary,sort_keys=True))
    return 0 if summary.get('status') == 'completed' else 2


if __name__=='__main__':
    raise SystemExit(main())
