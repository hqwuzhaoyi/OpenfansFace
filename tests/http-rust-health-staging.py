"""#17 × #8/#16: native scans race real staging, restore and permanent deletion."""
import importlib.util, json, time, uuid
from pathlib import Path
spec=importlib.util.spec_from_file_location('health',Path(__file__).with_name('http-rust-video-health.py'))
h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)

def stage(x):
    request=x.request();request['action']='delete-files';request.pop('filename');plan=x.preview(request);x.apply(plan)
    return x.call(route='/v1/staging')['rows'][0]
def operate(x,row,action):
    plan=x.preview({'id':str(uuid.uuid4()),'action':action,'person':'','files':[{key:row[key] for key in ('vid','path','version')}]})
    body={'mode':'apply','request':plan['request'],'version':plan['version']}
    if action=='delete-staged':body['confirmation']='永久删除'
    return x.call(body)
def scenario(restore):
    x=h.Fixture()
    try:
        # Save a complete initial report before the later long probe starts.
        x.run();x.done();identity=x.guard();original=x.source.read_bytes()
        marker=x.root/'health-probe-started'
        x.controlled(f"echo $$ > '{marker}'\n/bin/sleep 3\necho 'moov atom not found' >&2\nexit 1\n")
        x.run(h.NOW+40*86400)
        deadline=time.monotonic()+8
        while not marker.exists() and time.monotonic()<deadline:time.sleep(.03)
        assert marker.exists()
        staged=stage(x);assert not x.source.exists()
        result=operate(x,staged,'restore-files' if restore else 'delete-staged');assert result['ok'],result
        checked=x.done();assert checked['status']=='complete',checked
        stored=json.loads((x.base/'video-health.json').read_text())
        assert h.f.VID not in stored['records'],stored
        if restore:
            assert x.source.read_bytes()==original
            current=x.guard();assert current['fingerprint']==identity['fingerprint'],'restore fixture should retain inode and mtime to exercise material guard'
            assert current['material']!=identity['material']
            x.call({**identity,'code':3},'/v1/video-health',409)
        else:
            assert not Path(staged['path']).exists()
            x.call(route='/v1/video-health?vid='+h.f.urllib.parse.quote(h.f.VID,safe=''),status=404)
            x.run(h.NOW+41*86400,budget=0);latest=x.done();assert latest['total']==0 and latest['rows']==[],latest
        print('PASS scan publication rejects actual '+('same-inode same-path restoration; stale playback report fenced' if restore else 'permanent deletion; disposed ID stays out of next scan'))
    finally:x.stop()

if __name__=='__main__':scenario(False);scenario(True)
