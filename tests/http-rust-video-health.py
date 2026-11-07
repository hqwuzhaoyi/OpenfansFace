"""Native #17 isolated acceptance. Python only seeds/asserts; Rust owns all scans/writes."""
import concurrent.futures, importlib.util, json, os, shutil, sqlite3, time, uuid
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
NOW=1800000000
class Fixture(f.Fixture):
    def __init__(self):
        super().__init__()
        folder=self.media/'pikpak/My Telegram';folder.mkdir(parents=True);(folder/'sentinel').write_text('isolated')
        self.stop();self.env.update(PATH=os.environ['PATH'],VIDEO_HEALTH_ENABLED='0',VIDEO_HEALTH_FIXTURE_RUN_ON_START='1',VIDEO_HEALTH_FIXTURE_NOW=str(NOW));self.start()
    def guard(self,vid=f.VID):return self.call(route='/v1/video-health?vid='+f.urllib.parse.quote(vid,safe=''))
    def report(self,guard,oid=None):return self.call({**guard,'code':3,'operation_id':oid or str(uuid.uuid4())},'/v1/video-health')
    def run(self,clock=NOW,budget=300,seconds=1200):
        self.previous_run=json.loads((self.base/'video-health.json').read_text()).get('run_id') if (self.base/'video-health.json').exists() else None
        self.stop();self.env.update(VIDEO_HEALTH_ENABLED='1',VIDEO_HEALTH_FIXTURE_NOW=str(clock),VIDEO_HEALTH_BUDGET=str(budget),VIDEO_HEALTH_SECONDS=str(seconds));self.start()
    def done(self):
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            value=self.call(route='/v1/video-health')
            if value.get('status') in ('complete','skipped') and value.get('run_id')!=getattr(self,'previous_run',None):return value
            time.sleep(.05)
        raise AssertionError((self.root,(self.root/'service.log').read_text()))
    def controlled(self,script):
        path=self.root/'ffprobe-fixture';path.write_text('#!/bin/sh\n'+script);path.chmod(0o755);self.env['VIDEO_HEALTH_FIXTURE_PROBE']=str(path)

def actual_and_thresholds():
    x=Fixture()
    try:
        assert x.call(route='/v1/video-health')['status']=='never_run'
        x.call({'command':'health-start','body':{'run_id':'foreign'}},'/v1/metadata',400)
        x.call({'mode':'preview','request':{'id':str(uuid.uuid4()),'action':'metadata-write','person':'','files':[],'payload':{'command':'health-start','run_id':'foreign','at':'2026-01-01T00:00:00Z'}}},status=400)
        original=x.source.read_bytes()
        for t,checks,hidden in [(NOW,1,False),(NOW+20*3600-1,1,False),(NOW+20*3600,2,False),(NOW+40*3600,3,True)]:
            x.run(t);result=x.done();assert result['status']=='complete',result
            row=next(r for r in result['rows'] if r['vid']==f.VID);assert row['status']=='corrupt',row;assert row['consecutive']==checks and row['hidden']==hidden,row
        assert x.source.read_bytes()==original
        x.stop();x.env.update(VIDEO_HEALTH_ENABLED='0',VIDEO_HEALTH_FIXTURE_NOW=str(NOW+40*3600+7*86400));x.start()
        stale=x.call(route='/v1/video-health');assert not stale['rows'][0]['hidden'] and '7 天' in stale['error'],stale
        # Same ID/path/size but replacement inode must make old suppression ineffective immediately.
        replacement=x.source.with_suffix('.new');replacement.write_bytes(original);os.utime(replacement,ns=(x.source.stat().st_atime_ns,x.source.stat().st_mtime_ns));replacement.replace(x.source)
        assert not x.call(route='/v1/video-health')['rows'][0]['hidden']
        # Actual sample ffprobe, then playback-report priority, replay and replacement rejection.
        sample=Path('/tmp/persons-flower.mp4');assert sample.is_file(),'provide /tmp/persons-flower.mp4 playable sample'
        shutil.copyfile(sample,x.source);x.run(NOW+3*86400);result=x.done();assert result['counts']['healthy']==1,result
        identity=x.guard();oid=str(uuid.uuid4());x.report(identity,oid);x.report(identity,oid)
        with concurrent.futures.ThreadPoolExecutor(4) as pool:list(pool.map(lambda _:x.report(identity),range(4)))
        reports=json.loads((x.base/'video-health-reports.json').read_text());assert reports[f.VID]['count']==5,reports
        x.run(NOW+3*86400+10);result=x.done();assert result['rows'][0]['status']=='playback_warning' and not result['rows'][0]['hidden'],result
        x.source.write_bytes(b'replaced');x.call({**identity,'code':3},'/v1/video-health',409)
        print('PASS actual ffprobe, 3 / 20h / 40h policy, same-size inode replacement, concurrent reports/replay, playback warning, closed publish API')
    finally:x.stop()

def priority_timeout_and_moves():
    x=Fixture()
    try:
        second=x.source.with_name('priority.mp4');second.write_bytes(b'fixture');
        with sqlite3.connect(x.base/'faces.db') as db:db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)',('priority',str(second),second.name))
        identity=x.guard('priority');x.report(identity)
        log=x.root/'probed.log';x.controlled(f'''for path do :; done
printf '%s\\n' "$path" >> '{log}'
printf '%s' '{{"streams":[{{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p"}}]}}'
''')
        x.run(budget=1);result=x.done();assert log.read_text().strip()==str(second),(log.read_text(),result);assert result['waiting']==1;assert result['rows'][0]['status']=='playback_warning'
        # A long ffprobe cannot hold file-operation/status lane; late results may not follow rename.
        marker=x.root/'probe-started';x.controlled(f'''echo $$ > '{marker}'
/bin/sleep 2
printf '%s' '{{"streams":[{{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p"}}]}}'
''')
        x.run(NOW+40*86400,budget=2)
        deadline=time.monotonic()+8
        while not marker.exists() and time.monotonic()<deadline:time.sleep(.03)
        assert marker.exists()
        before=time.monotonic();plan=x.preview();assert x.apply(plan)['ok'];assert time.monotonic()-before<1.8
        result=x.done();assert all(r.get('path')!=str(x.source) for r in result['rows']),result
        marker.unlink();x.run(NOW+80*86400,budget=2,seconds=1);result=x.done();assert result['status']=='complete',result;assert any(r['status']=='unavailable' for r in result['rows']),result
        if marker.exists():
            pid=int(marker.read_text());
            try:os.kill(pid,0);raise AssertionError('probe not reaped')
            except ProcessLookupError:pass
        assert not list((x.base/'file-operations-rust').glob('probe-*'))
        print('PASS report priority, bounded timeout/reaped PID, independent writer/status, rename publication race')
    finally:x.stop()

def guards_restart_and_io():
    x=Fixture()
    try:
        x.controlled("echo 'Permission denied: moov atom not found' >&2\nexit 1\n");x.run();first=x.done();assert first['rows'][0]['status']=='unavailable' and not first['rows'][0]['hidden']
        previous=json.loads((x.base/'video-health.json').read_text())['records']
        # Empty expected mount directory must fail without replacing the previous complete report.
        (x.media/'pikpak/My Telegram/sentinel').unlink();x.run(NOW+86400);failed=x.done();assert failed['status']=='skipped',failed
        assert json.loads((x.base/'video-health.json').read_text())['records']==previous
        (x.media/'pikpak/My Telegram/sentinel').write_text('fixture')
        marker=x.root/'started';x.controlled(f"echo $$ > '{marker}'\n/bin/sleep 30\n")
        x.run(NOW+2*86400)
        deadline=time.monotonic()+8
        while not marker.exists() and time.monotonic()<deadline:time.sleep(.03)
        assert marker.exists();x.process.kill();x.process.wait(5);x.log.close()
        x.env['VIDEO_HEALTH_FIXTURE_RUN_ON_START']='0';x.start();state=x.call(route='/v1/video-health');assert state['status']=='interrupted',state
        assert json.loads((x.base/'video-health.json').read_text())['records']==previous
        # On macOS abrupt parent death has no PDEATHSIG: explicitly clean the isolated controlled fixture group.
        try:os.killpg(int(marker.read_text()),9)
        except ProcessLookupError:pass
        print('PASS permission uncertainty, mount failure retains previous results, real SIGKILL restart marks interrupted')
    finally:x.stop()

def mass_and_compatibility():
    x=Fixture()
    try:
        x.controlled("printf '%s' '{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"wmv3\",\"pix_fmt\":\"yuv420p\"}]}'\n")
        x.run();result=x.done();assert result['rows'][0]['status']=='compatibility' and not result['rows'][0]['hidden']
        before=json.loads((x.base/'video-health.json').read_text())['records']
        with sqlite3.connect(x.base/'faces.db') as db:
            for n in range(51):
                p=x.source.with_name(f'empty-{n}.mp4');p.write_bytes(b'');db.execute('INSERT INTO videos(vid,path) VALUES(?,?)',(f'empty-{n}',str(p)))
        x.run(NOW+86400);result=x.done();assert result['status']=='skipped' and 'max(50, 20%)' in result['error'],result
        assert json.loads((x.base/'video-health.json').read_text())['records']==before
        # Remove only this fixture's seeded empties, then make 51 nonempty probe-corrupt items.
        with sqlite3.connect(x.base/'faces.db') as db:
            for n in range(51):x.source.with_name(f'empty-{n}.mp4').write_bytes(b'broken fixture')
        x.controlled("echo 'moov atom not found' >&2\nexit 1\n")
        x.run(NOW+2*86400);result=x.done();assert result['status']=='skipped' and '探测后' in result['error'],result
        assert json.loads((x.base/'video-health.json').read_text())['records']==before
        print('PASS compatibility never hidden, mass anomaly both before and after probing retains complete prior records')
    finally:x.stop()

if __name__=='__main__':
    actual_and_thresholds();priority_timeout_and_moves();guards_restart_and_io();mass_and_compatibility()
