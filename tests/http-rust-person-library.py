"""Read-only inventory HTTP contract, exclusively marked /tmp fixtures.
Run: python3 tests/http-rust-person-library.py [--binary PATH]
"""
import importlib.util, json, sqlite3, uuid, urllib.parse, http.client, time
from datetime import datetime, timezone
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
fmod=importlib.util.module_from_spec(spec);spec.loader.exec_module(fmod)

def run():
    f=fmod.Fixture();clients=[];locked=None
    def listing(name):return f.call(route='/v1/person-library?'+urllib.parse.urlencode({'name':name}))
    def observations(ids):return f.call({'vids':ids},route='/v1/person-library/observations')['observations']
    try:
        root=f.source.parent
        nested=root/'子目录/更深';nested.mkdir(parents=True)
        extra=nested/'未扫描.MKV';extra.write_bytes(b'nested bytes')
        (root/'empty').mkdir();(root/'notes.txt').write_text('attachment')
        (root/'.hidden.mp4').write_bytes(b'hidden')
        (root/'.secret').mkdir();(root/'.secret/no.mp4').write_bytes(b'hidden nested')
        (root/'link.mp4').symlink_to(f.source)
        (root/'link-dir').symlink_to(nested,target_is_directory=True)
        (root.parent/'link-person').symlink_to(root,target_is_directory=True)
        names=f.call(route='/v1/person-library')['names'];assert names==['甲'],names
        inventory=listing('甲');assert inventory['directory_exists'] and inventory['root']==str(root)
        files={row['path']:row for row in inventory['files']}
        assert set(files)=={str(f.source),str(extra)},files
        assert files[str(f.source)]['vid']==fmod.VID
        assert files[str(extra)]['vid']==str(extra)
        assert inventory['folders']==[{'name':'.','count':1},{'name':'子目录/更深','count':1}],inventory
        stats=f.call(route='/v1/person-library?stats=1')['stats']['甲']
        assert stats=={'video_count':2,'total_bytes':len(fmod.PAYLOAD)+len(b'nested bytes')},stats
        missing=listing('不存在');assert not missing['directory_exists'] and not missing['files'] and not missing['folders']
        f.call(route='/v1/person-library?name=link-person',status=409)
        for name in ('../outside','.hidden','a/b','a\\b','甲\x00'):
            f.call(route='/v1/person-library?'+urllib.parse.urlencode({'name':name}),status=400)
        print('PASS Chinese nested inventory, exact bytes, actual folders, missing, hidden/symlink skip and unsafe roots')

        # Versions come from the library response exactly as the UI submits them.
        row=files[str(extra)]
        request={'id':str(uuid.uuid4()),'action':'rename-file','person':'甲','filename':'稳定中文.mp4','files':[{k:row[k] for k in ('vid','path','version')}]}
        request['filename']='稳定中文.MKV'
        plan=f.preview(request);assert plan['files'][0]['version']==row['version'];f.apply(plan)
        stable=plan['files'][0]['vids'][0];assert stable.startswith('file:')
        current=next(r for r in listing('甲')['files'] if r['vid']==stable)
        assert current['path']==plan['files'][0]['dst']
        # Old existing path cannot reclaim a persisted current override, even if the current file disappears.
        extra.write_bytes(b'recreated old')
        old=next(r for r in listing('甲')['files'] if r['path']==str(extra));assert old['vid']==str(extra)
        moved=Path(current['path']);temporary=moved.with_suffix('.hold');moved.rename(temporary)
        assert all(r['vid']!=stable for r in listing('甲')['files'])
        assert observations([stable])[stable]['status']=='missing'
        temporary.rename(moved)
        assert observations([stable])[stable]['path']==str(moved)
        print('PASS UI-selected native fingerprint, mutation stable ID and authoritative current-source precedence')

        # Evidence reads share the authoritative health/source model and never accept arbitrary paths.
        candidate=f.media/'pikpak/My Telegram/候选.mp4';candidate.parent.mkdir();candidate.write_bytes(b'candidate')
        with sqlite3.connect(f.base/'faces.db') as db:
            db.execute('INSERT INTO videos VALUES(?,?,?,?,?)',('candidate',str(candidate),candidate.name,str(candidate.parent),candidate.stat().st_size))
            db.execute('INSERT INTO videos VALUES(?,?,?,?,?)',('dir-entry',str(candidate.parent),candidate.parent.name,str(candidate.parent.parent),0))
        observed=observations(['candidate','dir-entry',fmod.VID,str(f.root/'ledger.db')])
        assert observed['candidate']['eligible'] and observed['candidate']['mtime_ns']==str(candidate.stat().st_mtime_ns)
        assert observed['dir-entry']=={'status':'not_file','eligible':False,'path':str(candidate.parent)},observed['dir-entry']
        assert not observed[fmod.VID]['eligible'] and observed[str(f.root/'ledger.db')]['status']=='missing'
        f.call({'vids':['x']*513},route='/v1/person-library/observations',status=400)
        f.call({'vids':[],'path':str(candidate)},route='/v1/person-library/observations',status=400)
        with sqlite3.connect(f.base/'faces.db') as db:
            db.execute('INSERT INTO videos VALUES(?,?,?,?,?)',('same-source',str(candidate),candidate.name,str(candidate.parent),candidate.stat().st_size))
        fmod.write(f.base/'integration-queue.json',[{'vid':'same-source','creator':'甲'}])
        assert observations(['candidate'])['candidate']['assigned']
        fmod.write(f.base/'integration-queue.json',[])
        health=f.call(route='/v1/video-health?vid=candidate')
        fmod.write(f.base/'video-health.json',{'records':{'candidate':{**health,'status':'corrupt','hidden':True}},'last_success_at':datetime.now(timezone.utc).isoformat()})
        assert observations(['candidate'])['candidate']['hidden']
        candidate.write_bytes(b'changed candidate')
        assert observations(['candidate'])['candidate']['eligible']
        (f.base/'video-health.json').unlink()
        deep=root.parent/'深度';deep.mkdir();leaf=deep
        for _ in range(65):leaf=leaf/'d';leaf.mkdir()
        f.call(route='/v1/person-library?name='+urllib.parse.quote('深度'),status=413)
        import shutil
        shutil.rmtree(deep)
        locked=root/'unreadable';locked.mkdir();(locked/'secret.mp4').write_bytes(b'x');locked.chmod(0)
        f.call(route='/v1/person-library?name='+urllib.parse.quote('甲'),status=403)
        stats=f.call(route='/v1/person-library?stats=1')['stats']['甲']
        assert stats['video_count'] is None and stats['total_bytes'] is None and stats['stats_error'],stats
        locked.chmod(0o755);locked=None
        print('PASS candidate scope, fingerprint-bound health hiding, depth/ID bounds and unreadable directory never reported as zero')

        large=root.parent/'大目录';large.mkdir()
        for index in range(2000):(large/f'{index:04}.mp4').write_bytes(b'bounded')
        assert len(listing('大目录')['files'])==2000
        assert f.call(route='/v1/person-library?stats=1')['stats']['大目录']=={'video_count':2000,'total_bytes':14000}
        f.fault('media_listing','pause',milliseconds=2500)
        for index in range(4):
            client=http.client.HTTPConnection('127.0.0.1',f.port,timeout=10)
            client.request('GET',f'/v1/person-library?name='+urllib.parse.quote('大目录')+f'&offset={index}',headers={'Authorization':'Bearer fixture-token'})
            clients.append(client)
        deadline=time.monotonic()+2
        while len(list(f.root.glob('media-read-*.started')))!=4:
            assert time.monotonic()<deadline;time.sleep(.01)
        for _ in range(12):assert f.call(route='/v1/person-library?stats=1',status=503)['code']=='reader_busy'
        assert f.call({'vids':['candidate']},route='/v1/person-library/observations',status=503)['code']=='reader_busy'
        assert f.call(route='/health')['ok']
        assert f.status(plan)['status']=='complete'
        for client in clients:client.close()
        assert f.call(route='/v1/person-library',status=503)['code']=='reader_busy'
        (f.root/'rust-fault.json').unlink()
        deadline=time.monotonic()+20
        while True:
            request=fmod.urllib.request.Request(f.url+'/v1/person-library',headers={'Authorization':'Bearer fixture-token'})
            try:
                with fmod.urllib.request.urlopen(request,timeout=2) as response:json.load(response)
                break
            except fmod.urllib.error.HTTPError as error:assert error.code==503 and json.load(error)['code']=='reader_busy'
            assert time.monotonic()<deadline,'real readers failed to release permits'
            time.sleep(.1)
        assert len(listing('大目录')['files'])==2000
        print('PASS 2000-file inventory, shared bounded media admission, disconnect permit retention and independent receipts')
    finally:
        if locked:locked.chmod(0o755)
        (f.root/'rust-fault.json').unlink(missing_ok=True)
        for client in clients:client.close()
        f.stop()
if __name__=='__main__':run()
