"""Marked worker boot against a closed native coordinator; no models or vectors."""
import http.server
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'maintenance'))
import processing_worker as worker
from processing_store import connect, set_setting


class WorkerBootstrapTests(unittest.TestCase):
    def test_closed_native_keeps_heartbeat_without_recovery_discovery_or_claims(self):
        with tempfile.TemporaryDirectory(prefix='marked-worker-bootstrap-') as tmp:
            base=Path(tmp);(base/'media').mkdir()
            db=connect(base)
            with db:set_setting(db,'paused',True)
            db.close()
            references=base/'processing/references.npz';references.write_bytes(b'prior reference sentinel')
            database=base/'processing/processing.db';before=database.read_bytes()
            events=[];heartbeats=[];failures=[]
            class Handler(http.server.BaseHTTPRequestHandler):
                def log_message(self,*args):pass
                def do_POST(self):
                    body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    events.append(body['command'])
                    if len(events)==1:
                        status,payload=503,{'code':'admission_closed','error':'closed fixture admission'}
                    else:
                        try:
                            heartbeat=json.loads((base/'processing/heartbeat.json').read_text())
                            heartbeats.append(heartbeat)
                            assert heartbeat['waiting_native_admission'] is True
                            assert heartbeat['native_ready'] is False and heartbeat['active']==0
                            assert heartbeat['at']>0 and events==['state','state']
                            assert database.read_bytes()==before
                            assert references.read_bytes()==b'prior reference sentinel'
                        except Exception as error:failures.append(error)
                        status,payload=200,{'version':'fixture-accepted-state'}
                    raw=json.dumps(payload).encode();self.send_response(status);self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
            server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
            thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            def called(name):
                def record(*args,**kwargs):events.append(name)
                return record
            try:
                with patch.dict(os.environ,{'FILE_OPERATIONS_RUST_URL':f'http://127.0.0.1:{server.server_port}'}), patch.object(sys,'argv',['worker','--base',str(base),'--root',str(base/'media'),'--once']), patch.object(worker,'recover_publications',side_effect=called('recover_publications')), patch.object(worker,'recover',side_effect=called('recover')), patch.object(worker,'discover',side_effect=called('discover')), patch.object(worker,'process_one',side_effect=called('claim')), patch.object(worker,'inference_status',return_value={'backend':'cpu','available':None}), patch.object(worker.signal,'signal'):
                    worker.main()
                self.assertFalse(failures,failures)
                self.assertEqual(len(heartbeats),1)
                self.assertEqual(events,['state','state','recover_publications','recover','discover','claim'])
                self.assertEqual(references.read_bytes(),b'prior reference sentinel')
            finally:
                server.shutdown();server.server_close();thread.join()

    def test_wrong_authorization_fails_without_recovery_or_discovery(self):
        with tempfile.TemporaryDirectory(prefix='marked-worker-bootstrap-auth-') as tmp:
            base=Path(tmp)
            with patch.dict(os.environ,{'FILE_OPERATIONS_RUST_URL':'http://unused.fixture'}), patch.object(sys,'argv',['worker','--base',str(base),'--once']), patch.object(worker,'native',side_effect=worker.PublicationError('wrong fixture token',401)) as native, patch.object(worker,'recover_publications') as recovery, patch.object(worker,'discover') as discover, patch.object(worker,'connect') as database, patch.object(worker.signal,'signal'):
                with self.assertRaises(worker.PublicationError):worker.main()
                native.assert_called_once_with('state',{'scope':{}},timeout=5)
                recovery.assert_not_called();discover.assert_not_called();database.assert_not_called()
            heartbeat=json.loads((base/'processing/heartbeat.json').read_text())
            self.assertEqual(heartbeat['at'],0)
            self.assertFalse(heartbeat['native_ready'])
            self.assertFalse((base/'processing/processing.db').exists())

    def test_signal_during_closed_bootstrap_exits_without_business_activity(self):
        with tempfile.TemporaryDirectory(prefix='marked-worker-bootstrap-stop-') as tmp:
            base=Path(tmp);handlers={}
            def refuse(*args,**kwargs):
                handlers[worker.signal.SIGTERM](worker.signal.SIGTERM,None)
                raise worker.PublicationError('closed fixture admission',503)
            with patch.dict(os.environ,{'FILE_OPERATIONS_RUST_URL':'http://unused.fixture'}), patch.object(sys,'argv',['worker','--base',str(base),'--once']), patch.object(worker,'native',side_effect=refuse), patch.object(worker,'recover_publications') as recovery, patch.object(worker,'discover') as discover, patch.object(worker,'connect') as database, patch.object(worker.signal,'signal',side_effect=lambda signum,handler:handlers.update({signum:handler})):
                worker.main()
                recovery.assert_not_called();discover.assert_not_called();database.assert_not_called()
            heartbeat=json.loads((base/'processing/heartbeat.json').read_text())
            self.assertEqual(heartbeat['at'],0)
            self.assertFalse((base/'processing/processing.db').exists())

if __name__=='__main__':unittest.main()
