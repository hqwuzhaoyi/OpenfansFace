"""CPU-only isolated dependency/portrait decoder check; no identity lookup."""
import argparse
import hashlib
import inspect
import json
import pathlib
import time

from services.gpu_inference.model import Detector
from services.gpu_inference.protocol import Binding, Fingerprint, ProtocolError, Request, now_ms


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--model',required=True)
    parser.add_argument('--model-sha256',required=True)
    parser.add_argument('--image',required=True)
    parser.add_argument('--recognition-model')
    parser.add_argument('--recognition-sha256')
    parser.add_argument('--image-sha256',required=True)
    args=parser.parse_args()
    if hashlib.sha256(pathlib.Path(args.model).read_bytes()).hexdigest()!=args.model_sha256:
        raise RuntimeError('detector_hash_mismatch')
    import cv2
    import numpy as np
    import onnxruntime as ort
    from insightface.model_zoo.scrfd import SCRFD
    cv2.setNumThreads(1)
    options=ort.SessionOptions();options.intra_op_num_threads=options.inter_op_num_threads=1
    session=ort.InferenceSession(args.model,sess_options=options,providers=['CPUExecutionProvider'])
    kwargs={'model_file':args.model,'session':session}
    if 'static_shape_sessions' in inspect.signature(SCRFD).parameters:kwargs['static_shape_sessions']=False
    detector=SCRFD(**kwargs);detector.prepare(ctx_id=0,input_size=(640,640),det_thresh=.5,nms_thresh=.4)
    adapter=Detector.__new__(Detector);adapter.detector=detector;adapter.size=640
    data=pathlib.Path(args.image).read_bytes()
    if hashlib.sha256(data).hexdigest()!=args.image_sha256:raise RuntimeError('fixture_hash_mismatch')
    image=cv2.imdecode(np.frombuffer(data,np.uint8),cv2.IMREAD_COLOR | cv2.IMREAD_IGNORE_ORIENTATION)
    height,width=image.shape[:2]
    binding=Binding('isolated-task','synthetic-video','isolated-batch','attempt-1','lease-1','generation-1',
                    now_ms()+60_000,Fingerprint(len(data),'0',args.image_sha256),'a'*64,'portrait-check-v1')
    request=Request('synthetic-portrait',binding,'a'*64,'scrfd-640-bgr8-det0.5-nms0.4-v1',now_ms()+30_000,
                    0,0,args.image_sha256,width,height,data)
    transported=Request.parse(request.wire())
    if transported.image!=data:raise RuntimeError('transport_bytes_changed')
    t=time.monotonic();direct,points=detector.detect(image);direct_ms=(time.monotonic()-t)*1000
    t=time.monotonic();faces=adapter.infer(transported);decoded_ms=(time.monotonic()-t)*1000
    if not len(direct) or len(direct)!=len(faces):raise RuntimeError('positive_fixture_not_detected')
    actual=np.array([face['bbox']+[face['score']] for face in faces])
    actual_points=np.array([face['keypoints'] for face in faces])
    if not np.allclose(actual,direct,rtol=1e-5,atol=1e-4) or not np.allclose(actual_points,points,rtol=1e-5,atol=1e-4):
        raise RuntimeError('portrait_geometry_changed')
    # Decode the maximum permitted portrait without asking the detector to process
    # a false identity. The synthetic blank stays anonymous and is not a reference.
    blank=np.zeros((4096,1280,3),np.uint8);ok,encoded=cv2.imencode('.png',blank)
    blob=encoded.tobytes()
    from dataclasses import replace
    boundary=replace(request,request_id='portrait-boundary',width=1280,height=4096,
                     image=blob,image_sha256=hashlib.sha256(blob).hexdigest())
    Request.parse(boundary.wire());adapter.infer(boundary)
    try:adapter.infer(replace(request,width=width-1))
    except ProtocolError:bad_dimensions_rejected=True
    else:raise RuntimeError('dimension_mismatch_accepted')
    recognition = None
    if args.recognition_model:
        if not args.recognition_sha256 or hashlib.sha256(pathlib.Path(args.recognition_model).read_bytes()).hexdigest()!=args.recognition_sha256:
            raise RuntimeError('recognition_hash_mismatch')
        if pathlib.Path(args.recognition_model).parent != pathlib.Path(args.model).parent:
            raise RuntimeError('models_must_share_fixture_pack')
        from insightface.app import FaceAnalysis
        from insightface.app.common import Face
        reference = FaceAnalysis(name=pathlib.Path(args.model).parent.name,
            root=str(pathlib.Path(args.model).parents[2]), allowed_modules=['detection','recognition'],
            providers=['CPUExecutionProvider'], sess_options=options)
        reference.prepare(ctx_id=-1,det_size=(640,640))
        reference_faces=reference.get(image)
        if len(reference_faces)!=len(faces):raise RuntimeError('reference_face_count_changed')
        differences=[]
        for original,geometry in zip(reference_faces,faces):
            transferred=Face(bbox=np.asarray(geometry['bbox'],dtype=np.float32),
                             kps=np.asarray(geometry['keypoints'],dtype=np.float32),det_score=geometry['score'])
            reference.models['recognition'].get(image,transferred)
            a,b=original.normed_embedding,transferred.normed_embedding
            if a.shape!=(512,) or b.shape!=(512,) or not np.isfinite(a).all() or not np.isfinite(b).all():
                raise RuntimeError('invalid_normalized_vector')
            if not np.allclose(a,b,rtol=1e-5,atol=1e-5):raise RuntimeError('recognition_handoff_changed')
            differences.append({'dimensions':512,'max_abs_difference':float(np.abs(a-b).max()),
                                'cosine':float(a@b),'norm_reference':float(np.linalg.norm(a)),
                                'norm_transferred':float(np.linalg.norm(b))})
        recognition={'model_sha256':args.recognition_sha256,'comparisons':differences,
                     'reference_lookup_performed':False,'raw_vectors_recorded':False}
    print(json.dumps({'scope':'CPU-only synthetic positive portrait; optional vector numerical parity, no identity lookup',
        'model_sha256':args.model_sha256,'recognition_parity':recognition,
        'providers':session.get_providers(),'image_sha256':args.image_sha256,'width':width,'height':height,
        'transport_preserves_bytes':True,'positive_face_count':len(faces),'faces':faces,
        'boxes_max_abs_difference':float(np.abs(actual-direct).max()),
        'landmarks_max_abs_difference':float(np.abs(actual_points-points).max()),
        'cpu_direct_ms':direct_ms,'cpu_decode_and_detect_ms':decoded_ms,
        'max_portrait_raster':[1280,4096],'max_portrait_decode_passed':True,
        'invalid_dimensions_rejected':bad_dimensions_rejected},sort_keys=True))


if __name__=='__main__':main()
