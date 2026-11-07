"""Protocol simulator only. It never performs GPU inference or production work."""
import json,os,sqlite3,signal,sys
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from pathlib import Path
signal.signal(signal.SIGTERM,lambda *_:sys.exit(0))
ROOT=Path(os.environ['REHEARSAL_ROOT'])
assert (ROOT/'.release-rehearsal').read_text()=='isolated controller rehearsal'
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  if self.path=='/health/ready':
   if self.headers.get('Authorization')!='Bearer rehearsal-only-token':self.send_error(401);return
   data={'simulated':True,'live':True,'model_ready':True,'accepting':True,'draining':False,'service_instance':'simulated-local-peer','execution':{'cuda_kernel_events':1},'release':{'managed':True,'release_id':'SIMULATED-NOT-GPU-ACCEPTANCE'}}
  elif self.path=='/api/rebuild':data={'running':False,'simulated':True}
  elif self.path=='/playback':
   payload=b'isolated protocol fixture';self.send_response(206);self.send_header('Content-Range',f'bytes 0-{len(payload)-1}/{len(payload)}');self.end_headers();self.wfile.write(payload);return
  else:data={'ok':True,'simulated':True,'path':self.path}
  self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(json.dumps(data).encode())
 def do_POST(self):
  data=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))))
  if self.path=='/api/processing' and data.get('action')=='pause':
   with sqlite3.connect(ROOT/'base/processing/processing.db') as db:db.execute("INSERT OR REPLACE INTO settings VALUES('paused','true')")
  self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"ok":true,"simulated":true}')
ThreadingHTTPServer(('0.0.0.0',8080),Handler).serve_forever()
