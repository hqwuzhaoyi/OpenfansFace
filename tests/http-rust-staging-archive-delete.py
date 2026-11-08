"""Joint #6/#7/#8/#16 actual archive-produced origins, canonical rename and disposal."""
import importlib.util,json,sqlite3,urllib.request,urllib.error,uuid,hashlib
from pathlib import Path
spec=importlib.util.spec_from_file_location('resolution',Path(__file__).with_name('http-rust-archive-resolution.py'))
n=importlib.util.module_from_spec(spec);spec.loader.exec_module(n)
def call(f,body=None,route='/v1/person-files',status=200):
    request=urllib.request.Request(f.url+route,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer archive-fixture'})
    try:
        with urllib.request.urlopen(request,timeout=60) as response:code,data=response.status,json.load(response)
    except urllib.error.HTTPError as error:code,data=error.code,json.load(error)
    assert code==status,(code,data)
    return data
def pick(row):return {k:row[k] for k in ('vid','path','version')}
def file(path,vid):
    st=path.stat();return {'vid':vid,'path':str(path),'version':hashlib.sha256(json.dumps([str(path),str(st.st_dev),str(st.st_ino),str(st.st_size),str(st.st_mtime_ns)],ensure_ascii=False,separators=(',',':')).encode()).hexdigest()}
f,targets=n.prepare(2)
try:
    for i in range(2):
        with sqlite3.connect(f.base/'faces.db') as db:db.execute('INSERT INTO faces VALUES(?,?,?)',(f'canonical:{i}',f'canonical-thumb-{i}',.9))
    n.resolve(f,'stage-duplicate');archived=f.apply();n.assert_staged(f,targets,archived)
    rows=call(f,route='/v1/staging')['rows'];assert {row['vid'] for row in rows}==set(f.ids),rows
    original_rows={row['vid']:row for row in rows}
    request={'id':str(uuid.uuid4()),'action':'rename-file','person':'Momoyih','files':[file(targets[0],'canonical:0')],'filename':'canonical-renamed.mp4'}
    rename=call(f,{'mode':'preview','request':request});assert rename['files'][0]['vids']==['canonical:0'],rename['files']
    call(f,{'mode':'apply','request':rename['request'],'version':rename['version']});renamed=Path(rename['files'][0]['dst'])
    row=original_rows[f.ids[0]];delete=call(f,{'mode':'preview','request':{'id':str(uuid.uuid4()),'action':'delete-staged','person':'','files':[pick(row)]}})
    assert delete['files'][0]['vids']==[f.ids[0]],delete['files']
    deleted=call(f,{'mode':'apply','request':delete['request'],'version':delete['version'],'confirmation':'永久删除'})
    assert deleted['completed']==1 and not Path(row['path']).exists() and renamed.read_bytes()==n.n.BYTES+b'0'
    with sqlite3.connect(f.ledger) as db:
        assert db.execute('SELECT path,disposal FROM asset WHERE id=100').fetchone()==(f.win(renamed),None)
        assert db.execute('SELECT disposal FROM asset WHERE id=1').fetchone()==('trash',)
        assert db.execute('SELECT count(*) FROM asset_entity WHERE asset_id=100').fetchone()==(1,)
    with sqlite3.connect(f.base/'faces.db') as db:
        assert db.execute('SELECT path FROM videos WHERE vid=?',('canonical:0',)).fetchone()==(str(renamed),)
        assert db.execute('SELECT thumb FROM faces WHERE vid=?',('canonical:0',)).fetchone()==('canonical-thumb-0',)
        assert db.execute('SELECT path FROM videos WHERE vid=?',(f.ids[0],)).fetchone()==(row['path'],)
    other=original_rows[f.ids[1]];restore=call(f,{'mode':'preview','request':{'id':str(uuid.uuid4()),'action':'restore-files','person':'','files':[pick(other)]}})
    call(f,{'mode':'apply','request':restore['request'],'version':restore['version']})
    assert f.sources[1].read_bytes()==targets[1].read_bytes()==n.n.BYTES+b'1'
    with sqlite3.connect(f.ledger) as db:assert db.execute('SELECT path,disposal FROM asset WHERE id=101').fetchone()==(f.win(targets[1]),None)
    assert call(f,route='/v1/staging')['files']==0
    print('PASS actual duplicate archive origins → canonical rename → selected irreversible deletion + other duplicate restore; canonical bytes/assets/links/IDs/thumbnails preserved')
finally:f.stop()
