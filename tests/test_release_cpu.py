"""Retained CPU is explicit and bounded; default GPU release gates stay strict."""
import copy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import tempfile
import sys
import time
import unittest
from unittest.mock import patch
from test_release_control import r

ROOT=Path(__file__).parents[1]
SOURCE_NAMES=('processing_worker.py','processing_faces.py','processing_gpu.py','watermark_worker.py','processing_publication.py')

def ref(path):return {'path':str(path),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}

class CpuReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='marked-cpu-release-');self.root=Path(self.temp.name)
        self.m=json.loads((ROOT/'deployment/native-release.manifest.example.json').read_text())
        self.m.update(inference_backend='cpu',base=str(self.root));self.m.pop('gpu_peer')
        self.worker=self.m['services']['worker'];self.worker['candidate_image']='sha256:fixture'
        self.envfile=self.root/'worker.env';self.envfile.write_text('PROCESSING_FACE_BACKEND=cpu\nPROCESSING_MAX_CONCURRENCY=1\n')
        self.worker['create_args']=['--name',self.worker['name'],'--cpus','1','--memory','1536m','--env-file',str(self.envfile),'sha256:fixture']
        self.image={'Id':'sha256:fixture','Config':{'Env':['PROCESSING_FACE_BACKEND=gpu-detection','PROCESSING_MAX_CONCURRENCY=2']}}
        self.decision={'user_instruction':'retain current CPU resources and verify','runtime':{'cpu_limit':1,'memory_bytes':1610612736,'concurrency':1,'ocr_device':'cpu','face_provider':'CPUExecutionProvider','candidate_env':{'PROCESSING_FACE_BACKEND':'cpu','PROCESSING_MAX_CONCURRENCY':'1'}}}
        self.proof={'status':'retained_cpu_isolated_validation_passed','source_head':'a'*40,
          'configuration':{'backend':'cpu','cpu_limit':1,'memory_mib':1536,'concurrency':1,'network':'none','production_media_mounted':False},
          'checks':{'ocr':{'sampled_frames':8,'expected_watermark_found':True},'face':{'sampled_frames':8,'face_count':8,'dimension':512,'norm_min':1.0,'norm_max':1.0,'execution':{'model_hashes':{'det_10g.onnx':'5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91','w600k_r50.onnx':'4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43'},'backend':'cpu','detection_provider':'CPUExecutionProvider','recognition_provider':'CPUExecutionProvider'}},'processing_tests':40,'dashboard_model_tests':5,'native_worker_review_joint':'passed; mocked model stages with real native HTTP'}}
        self.sources={'head':'a'*40,'files':{'source/'+name:'b'*64 for name in SOURCE_NAMES}}
        self.review={'version':1,'purpose':'retain-production-cpu','release_id':self.m['release_id'],'verified_worker_sources':{name:'b'*64 for name in SOURCE_NAMES}}
        self.freeze()

    def tearDown(self):self.temp.cleanup()

    def freeze(self):
        for key,value in (('user_decision',self.decision),('verification',self.proof),('source_manifest',self.sources)):
            path=self.root/(key+'.json');path.write_text(json.dumps(value));self.review[key]=ref(path)
        with patch.object(r,'run',return_value=json.dumps([self.image])):self.review['worker_configuration']=r.cpu_worker_configuration(self.m)
        path=self.root/'review.json';path.write_text(json.dumps(self.review));self.m['cpu_release']=ref(path)

    def gate(self):
        with patch.object(r,'run',return_value=json.dumps([self.image])):return r.cpu_release_gate(self.m)

    def test_exact_cpu_proof_and_frozen_worker_configuration_pass_without_gpu(self):
        self.assertEqual(self.gate()['backend'],'cpu')
        self.assertNotIn('gpu_full_640_coexistence',r.release_gates(self.m))
        self.assertEqual(set(r.release_gates(self.m)),set(r.GATES)-{'gpu_full_640_coexistence'})
        with patch.object(r,'run',return_value=json.dumps([self.image])),patch.object(r.urllib.request,'urlopen') as request:
            self.assertEqual(r.gpu_probe(self.m)['native_joint_models'],'mocked');request.assert_not_called()

    def test_missing_or_unknown_backend_preserves_gpu_strictness(self):
        gpu=copy.deepcopy(self.m);gpu.pop('inference_backend')
        self.assertEqual(r.release_gates(gpu),r.GATES)
        with self.assertRaisesRegex(RuntimeError,'GPU requires'):r.validate(gpu,release=True)
        for value in ('automatic','CPU',None):
            with self.assertRaisesRegex(RuntimeError,'unknown'):r.validate({**self.m,'inference_backend':value})

    def test_user_decision_actual_evidence_source_and_review_hashes_cannot_be_faked(self):
        for field in ('generic','gpu','budget','source','simulated','hash'):
            original=copy.deepcopy((self.proof,self.sources))
            if field=='generic':self.proof={'passed':True}
            elif field=='gpu':self.proof['checks']['face']['execution']['detection_provider']='CUDAExecutionProvider'
            elif field=='budget':self.proof['configuration']['memory_mib']=2048
            elif field=='source':self.sources['head']='c'*40
            elif field=='simulated':self.proof['simulated']=True
            self.freeze()
            if field=='hash':self.m['cpu_release']['sha256']='0'*64
            with self.assertRaises(RuntimeError):self.gate()
            self.proof,self.sources=original;self.freeze()

    def test_gpu_or_changed_env_and_resource_overrides_fail_before_mutation(self):
        for extra in (['--env','PROCESSING_FACE_BACKEND=gpu-detection'],['-ePROCESSING_FACE_BACKEND=gpu-detection'],['--cpus','2'],['-m','2g'],['--gpus','all']):
            original=self.worker['create_args'][:];self.worker['create_args'][-1:-1]=extra
            with patch.object(r,'run',return_value=json.dumps([self.image])),self.assertRaises(RuntimeError):r.cpu_release_gate(self.m)
            self.worker['create_args']=original
        self.envfile.write_text('PROCESSING_FACE_BACKEND=gpu-detection\nPROCESSING_MAX_CONCURRENCY=1\n')
        with self.assertRaises(RuntimeError):self.gate()

    def test_live_candidate_requires_explicit_cpu_and_real_host_budget(self):
        state={'Config':{'Env':['PROCESSING_FACE_BACKEND=cpu','PROCESSING_MAX_CONCURRENCY=1']},'HostConfig':{'NanoCpus':1000000000,'Memory':1610612736}}
        r.cpu_worker_live(self.m,state,candidate=True)
        old=copy.deepcopy(state);old['Config']['Env']=[];r.cpu_worker_live(self.m,old,candidate=False)
        with self.assertRaises(RuntimeError):r.cpu_worker_live(self.m,old,candidate=True)
        for field,value in (('NanoCpus',2000000000),('Memory',2**30),('DeviceRequests',[{'Driver':'nvidia'}])):
            bad=copy.deepcopy(state);bad['HostConfig'][field]=value
            with self.assertRaises(RuntimeError):r.cpu_worker_live(self.m,bad,candidate=True)

    def test_waiting_bootstrap_is_not_postopen_cpu_semantic_readiness(self):
        path=self.root/'processing/heartbeat.json';path.parent.mkdir()
        hb={'at':time.time(),'protocol':3,'waiting_native_admission':True,'native_ready':False,'active':0}
        path.write_text(json.dumps(hb));self.assertTrue(r.cpu_worker_heartbeat(self.m));self.assertIsNone(r.cpu_worker_heartbeat(self.m,False))
        hb.update(waiting_native_admission=False,native_ready=True,inference={'backend':'cpu','fallback':False});path.write_text(json.dumps(hb));self.assertTrue(r.cpu_worker_heartbeat(self.m,False))
        hb['inference']['backend']='gpu-detection';path.write_text(json.dumps(hb))
        with self.assertRaisesRegex(RuntimeError,'explicit CPU'):r.cpu_worker_heartbeat(self.m,False)

    def test_cpu_release_still_requires_each_non_gpu_gate(self):
        with patch.object(r,'run',return_value=json.dumps([self.image])):
            with self.assertRaisesRegex(RuntimeError,'isolated_contracts'):r.validate(self.m,release=True)
        self.m['gpu_peer']={'adapter':'external-systemd'}
        with self.assertRaisesRegex(RuntimeError,'remove'):r.validate(self.m)

    def test_failed_postopen_cpu_readiness_recloses_native(self):
        self.m['deploy_lock']=str(self.root/'deploy.lock')
        file=self.root/'manifest.json';file.write_text(json.dumps(self.m))
        def api(manifest,path,body=None):
            if path=='/v1/readiness':return {'drained':True,'schema':9,'authority':self.m['authority_identity']}
            return {'accepting':body['accepting']}
        with patch.object(sys,'argv',['controller','open','--manifest',str(file),'--execute']), patch.object(r,'validate'), patch.object(r,'preserved_runtime'), patch.object(r,'baseline'), patch.object(r,'legacy_health_gate'), patch.object(r,'authority'), patch.object(r,'probes'), patch.object(r,'wait_cpu_worker_ready',side_effect=RuntimeError('wrong CPU runtime')), patch.object(r,'api',side_effect=api) as calls:
            with self.assertRaisesRegex(RuntimeError,'wrong CPU runtime'):r.main()
            admissions=[call.args[2] for call in calls.call_args_list if call.args[1]=='/v1/admission']
            self.assertEqual(admissions,[{'accepting':True},{'accepting':False}])

    def running_candidate(self, started):
        return {'Id':self.worker['expected_id'],'Image':self.worker['candidate_image'],
                'Config':{'Env':['PROCESSING_FACE_BACKEND=cpu','PROCESSING_MAX_CONCURRENCY=1']},
                'HostConfig':{'NanoCpus':1000000000,'Memory':1610612736},
                'State':{'Running':True,'StartedAt':started}}

    def write_heartbeat(self, stamp, waiting=False):
        path=self.root/'processing/heartbeat.json';path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps({'at':stamp,'protocol':3,'waiting_native_admission':waiting,
          'native_ready':not waiting,'active':0,'inference':{'backend':'cpu','fallback':False}}))

    def test_preopen_wait_rejects_predecessor_then_accepts_current_waiting_sample(self):
        now=time.time();start=datetime.fromtimestamp(now,timezone.utc).strftime('%Y-%m-%dT%H:%M:%S')+'.123456789Z'
        started=datetime.fromisoformat(start.split('.')[0]+'+00:00').timestamp()+.123456789
        state=self.running_candidate(start);self.write_heartbeat(now-20)
        self.m['startup_timeout']=0
        with patch.object(r,'inspect',return_value=state),self.assertRaisesRegex(RuntimeError,'fresh current-container'):
            r.wait_cpu_worker_ready(self.m,allow_waiting=True)
        self.m['startup_timeout']=30
        with patch.object(r,'inspect',return_value=state) as inspected,patch.object(r.time,'sleep',side_effect=lambda _:self.write_heartbeat(max(now,started)+.1,waiting=True)):
            r.wait_cpu_worker_ready(self.m,allow_waiting=True)
            self.assertEqual(inspected.call_count,2)

    def test_postopen_wait_requires_new_sample_after_open_boundary(self):
        now=time.time();start=datetime.fromtimestamp(now-20,timezone.utc).isoformat()
        state=self.running_candidate(start);self.write_heartbeat(now-1)
        with patch.object(r,'inspect',return_value=state) as inspected,patch.object(r.time,'sleep',side_effect=lambda _:self.write_heartbeat(now+.1)):
            r.wait_cpu_worker_ready(self.m,not_before=now)
            self.assertEqual(inspected.call_count,2)

    def test_old_ready_sample_times_out_and_open_recloses_without_current_ack(self):
        now=time.time();state=self.running_candidate(datetime.fromtimestamp(now-20,timezone.utc).isoformat())
        self.write_heartbeat(now-1);self.m.update(startup_timeout=0,deploy_lock=str(self.root/'deploy.lock'))
        file=self.root/'manifest-old-heartbeat.json';file.write_text(json.dumps(self.m))
        def api(manifest,path,body=None):
            if path=='/v1/readiness':return {'drained':True,'schema':9,'authority':self.m['authority_identity']}
            return {'accepting':body['accepting']}
        with patch.object(sys,'argv',['controller','open','--manifest',str(file),'--execute']),patch.object(r,'validate'),patch.object(r,'preserved_runtime'),patch.object(r,'baseline'),patch.object(r,'legacy_health_gate'),patch.object(r,'authority'),patch.object(r,'probes'),patch.object(r,'inspect',return_value=state),patch.object(r,'api',side_effect=api) as calls:
            with self.assertRaisesRegex(RuntimeError,'fresh current-container'):r.main()
            admissions=[call.args[2] for call in calls.call_args_list if call.args[1]=='/v1/admission']
            self.assertEqual(admissions,[{'accepting':True},{'accepting':False}])

    def test_reclassification_journals_block_active_recovery_and_unknown_in_cpu_mode(self):
        path=self.root/'reclassification-operations/00000000-0000-4000-8000-000000000001/result.json'
        path.parent.mkdir(parents=True)
        with self.assertRaisesRegex(RuntimeError,'unresolved'):r.legacy_journals(self.m)
        for value in ({'status':'running'},{'status':'recovery_needed'},{'status':'unknown'},
                      {'status':'complete','result':{'status':'recovery_needed'}},
                      {'status':'partial','recovery_errors':['fixture unknown outcome']}):
            path.write_text(json.dumps(value));self.m['legacy_terminal_acknowledgements']={str(path.relative_to(self.root)):ref(path)['sha256']}
            with self.assertRaisesRegex(RuntimeError,'unresolved'):r.legacy_journals(self.m)
        for value in ({'status':'complete'},{'state':'complete','completed':149,'total':149,'errors':[]},{'state':'partial'}):
            path.write_text(json.dumps(value));self.assertEqual(len(r.legacy_journals(self.m)),1)
        path.write_bytes(b'x'*(r.CONTROL_LIMIT+1))
        with self.assertRaises(RuntimeError):r.legacy_journals(self.m)

if __name__=='__main__':unittest.main()
