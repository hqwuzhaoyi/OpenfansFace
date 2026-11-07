"""HTTP privacy guard with a marked fixture receipt; native receipt creation tested separately."""
import hashlib,json,os,sqlite3,urllib.error,urllib.parse,urllib.request,uuid
from pathlib import Path
root=Path(os.environ.get('PROCESSING_DASHBOARD_FIXTURE','/tmp/openfansface-rust-implementation/worker19-fixture')).resolve()
assert root.name=='worker19-fixture' and (root/'.fixture-marker').read_text()=='worker19-isolated'
origin=os.environ.get('PROCESSING_DASHBOARD_URL','http://127.0.0.1:8969');assert origin.startswith('http://127.0.0.1:')
base=root/'base';attempt=str(uuid.uuid4());directory=base/'processing/attempts'/attempt/'thumbs';directory.mkdir(parents=True)
image=directory/'fixture.jpg';image.write_bytes(b'isolated thumbnail fixture')
source=root/'media/pikpak/fixture/fixture-0.mp4';stat=source.stat()
def status(file):
    try:
        with urllib.request.urlopen(origin+'/thumb?p='+urllib.parse.quote(str(file),safe=''),timeout=10) as response:return response.status,response.read()
    except urllib.error.HTTPError as error:return error.code,error.read()
variants=[image,Path('/tmp')/image.relative_to('/private/tmp') if str(image).startswith('/private/tmp/') else image,directory/'..'/'thumbs'/'fixture.jpg']
for file in variants:assert status(file)[0]==404,(file,status(file)[0])
receipt={'ok':True,'accepted':True,'vid':'fixture-0','task':{'run_id':attempt,'path':str(source),'size':stat.st_size,'mtime_ns':str(stat.st_mtime_ns)},'source_identity':{'dev':str(stat.st_dev),'ino':str(stat.st_ino)},'data':{'face':{'thumbs':[str(image)]}}}
con=sqlite3.connect(base/'processing/processing.db')
try:
    con.execute('CREATE TABLE IF NOT EXISTS _face_admin_processing_results(id TEXT PRIMARY KEY,signature TEXT NOT NULL,result TEXT NOT NULL)')
    con.execute('INSERT INTO _face_admin_processing_results VALUES(?,?,?)',(attempt,'marked-fixture',json.dumps(receipt)));con.commit()
    for file in variants:assert status(file)==(200,image.read_bytes()),file
    other=directory/'unreferenced.jpg';other.write_bytes(b'private');assert status(other)[0]==404
    con.execute('DELETE FROM _face_admin_processing_results WHERE id=?',(attempt,));con.commit()
    assert status(image)[0]==404
finally:
    con.execute('DELETE FROM _face_admin_processing_results WHERE id=?',(attempt,));con.commit();con.close()
print('PASS unaccepted / aliased / undeclared attempt image guards and accepted native-receipt read model')
