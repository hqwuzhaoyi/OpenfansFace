"""Same-origin #13 Next adapters against a compiled app and an isolated native executor."""
import importlib.util,json,os,socket,subprocess,time,urllib.request,urllib.error,urllib.parse,datetime,shutil
from pathlib import Path
spec=importlib.util.spec_from_file_location('shared',Path(__file__).with_name('http-rust-shared-metadata.py'))
s=importlib.util.module_from_spec(spec);spec.loader.exec_module(s)
x=s.Fixture();process=None
try:
    with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
    url=f'http://127.0.0.1:{port}'
    cache=json.loads((x.base/'external-profiles/nv-pu-sa.json').read_text());cache['fetched_at']=datetime.datetime.now(datetime.timezone.utc).isoformat();s.f.write(x.base/'external-profiles/nv-pu-sa.json',cache)
    env={**os.environ,**{key:value for key,value in x.env.items() if key!='PATH'},'FILE_OPERATIONS_BACKEND':'rust','FILE_OPERATIONS_ENABLE_RUST':'1','FILE_OPERATIONS_RUST_URL':x.url}
    log=(x.root/'next.log').open('w')
    process=subprocess.Popen([shutil.which('node'),str(s.f.REPO/'node_modules/next/dist/bin/next'),'start','-p',str(port)],cwd=s.f.REPO,env=env,stdout=log,stderr=log)
    def call(route,body=None,method=None,status=200):
        request=urllib.request.Request(url+route,data=json.dumps(body).encode() if body is not None else None,method=method,headers={'Content-Type':'application/json'})
        try:
            with urllib.request.urlopen(request,timeout=30) as response:code,value=response.status,json.load(response)
        except urllib.error.HTTPError as error:code,value=error.code,json.load(error)
        assert code==status,(code,status,value,x.root)
        return value
    for _ in range(300):
        if process.poll() is not None:raise AssertionError((x.root/'next.log').read_text())
        try:call('/api/persons/accounts?name='+urllib.parse.quote('甲'));break
        except OSError:time.sleep(.05)
    link=call('/api/persons/accounts',{'name':'旧甲','profile_id':'123','handle':'creator'})['link']
    rows=call('/api/persons/accounts?name='+urllib.parse.quote('甲'))['links'];assert len(rows)==1 and rows[0]['id']==link['id']
    call('/api/persons/accounts',{'name':'甲','profile_id':'123','handle':'mismatch'},status=409)
    call('/api/persons/accounts',{'name':'甲','id':'foreign'},'DELETE',409)
    call('/api/persons/accounts',{'name':'旧甲','id':link['id']},'DELETE');assert x.links()==[]
    reject=call('/api/persons/recommendations',{'name':'甲','vid':'candidate'})
    assert json.loads((x.base/'person-rejections.json').read_text())[json.dumps(['甲','candidate'],ensure_ascii=False,separators=(',',':'))]['id']==reject['id']
    call('/api/persons/recommendations',{'name':'甲','vid':'candidate','id':reject['id']},'DELETE')
    key='a'*64;candidate={'key':key,'text':'@creator','token':'creator','platform':'X','kind':'account','frames':3};stat=x.video.stat()
    s.f.write(x.base/'watermarks/results.json',{'candidate':{'path':str(x.video),'size':stat.st_size,'mtime_ns':str(stat.st_mtime_ns),'status':'done','candidates':[candidate]}})
    result=call('/api/watermarks',{'key':key,'kind':'person','creator':'甲'},'PUT');assert result['ok']
    assert json.loads((x.base/'watermarks/mappings.json').read_text())[key]['creator']=='甲'
    call('/api/watermarks',{'key':key,'kind':'remove'},'PUT')
    assert not json.loads((x.base/'watermarks/mappings.json').read_text())
    # Existing accounts API must report failed native save, never fall back to Node.
    (x.base/'.review-write.lock').write_text('{"fixture":"busy"}')
    failed=call('/api/persons/accounts',{'name':'甲','profile_id':'123','handle':'creator'},status=409)
    assert failed['operation_id'] and not x.links()
    print('PASS compiled Next same-origin accounts/save/unlink, rejection/undo, mapping/remove, native conflict + operation ID')
    print('Fixture evidence:',x.root)
finally:
    if process:
        process.terminate();process.wait(10);log.close()
    x.stop()
