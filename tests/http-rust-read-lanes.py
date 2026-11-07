"""Slow media cannot starve local receipt reads or spawn unbounded blocking work.
Only the inherited fully marked /tmp fixture can activate media_listing pause.
Run: python3 tests/http-rust-read-lanes.py [--binary PATH] [--port ISOLATED_PORT]
"""
import http.client
import importlib.util
import json
import uuid
from pathlib import Path
import time

spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('http-rust-file-operations.py'))
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)


def run():
    f=fixture.Fixture();clients=[]
    try:
        plan=f.preview()
        f.fault('media_listing','pause',milliseconds=10000)
        for index in range(4):
            client=http.client.HTTPConnection('127.0.0.1',f.port,timeout=15)
            client.request('GET',f'/v1/staging?offset={index}',headers={'Authorization':'Bearer fixture-token'})
            clients.append(client)
        deadline=time.monotonic()+3
        while len(list(f.root.glob('media-read-*.started')))!=4:
            assert time.monotonic()<deadline,('four media readers not admitted',f.root)
            time.sleep(.01)
        markers={p.name for p in f.root.glob('media-read-*.started')}
        assert markers=={f'media-read-{index}.started' for index in range(4)}

        # Auth/validation precede lane admission; saturation cannot hide these errors.
        start=time.monotonic()
        f.call(route='/v1/staging',status=401,token=False)
        f.call(route='/v1/staging?offset=invalid',status=400)
        assert time.monotonic()-start<1
        # Every excess request is rejected before spawn_blocking, not queued in its pool.
        for index in range(4,20):
            start=time.monotonic()
            value=f.call(route=f'/v1/staging?offset={index}',status=503)
            assert value['code']=='reader_busy' and time.monotonic()-start<1,value
        assert {p.name for p in f.root.glob('media-read-*.started')}==markers

        old_id = str(uuid.uuid4())
        receipt = f.base/'staging-operations'/old_id/'operation.json'
        receipt.parent.mkdir(parents=True)
        receipt.write_text(json.dumps({'id':old_id, 'status':'complete', 'result':{'ok':True,'fixture':'historical'}}))
        latencies=[]
        for _ in range(3):
            start=time.monotonic()
            status=f.status(plan)
            assert status['status']=='not_started' and status['id']==plan['id'],status
            assert f.call(route='/v1/archive')=={'job':None}
            assert f.call(route='/health')['ok']
            assert f.call(route='/v1/staging?id='+old_id)['result']['fixture'] == 'historical'
            latencies.append(time.monotonic()-start)
        assert max(latencies)<1,latencies
        print('PASS all four media permits occupied: file receipt/archive progress/health responsive',latencies)

        # Abandoning all HTTP waiters must not free permits while their syscall stand-ins run.
        for client in clients:client.close()
        time.sleep(.05)
        assert f.call(route='/v1/staging?offset=999',status=503)['code']=='reader_busy'
        assert {p.name for p in f.root.glob('media-read-*.started')}==markers
        print('PASS disconnected media waiters retain permits; 17 excess reads spawn no blocking work')

        # Release test delay for new work, then wait only for real existing readers to finish.
        (f.root/'rust-fault.json').unlink()
        deadline=time.monotonic()+12
        while True:
            request=fixture.urllib.request.Request(f.url+'/v1/staging?offset=1000',headers={'Authorization':'Bearer fixture-token'})
            try:
                with fixture.urllib.request.urlopen(request,timeout=2) as response:
                    value=json.load(response);assert value['files']==0 and value['rows']==[]
                    break
            except fixture.urllib.error.HTTPError as error:
                assert error.code==503 and json.load(error)['code']=='reader_busy'
            assert time.monotonic()<deadline,'media permits never released'
            time.sleep(.05)
        assert f.status(plan)['status']=='not_started'
        print('PASS media capacity recovers after actual read completion; receipt unchanged')
        # Fault files alone cannot activate the hook without the full fixture configuration.
        f.stop();f.env.pop('FILE_OPERATIONS_FIXTURE_ROOT');f.fault('media_listing','pause',milliseconds=10000);f.start()
        start=time.monotonic()
        assert f.call(route='/v1/staging?offset=1001')['files']==0
        assert time.monotonic()-start<1
        assert {p.name for p in f.root.glob('media-read-*.started')}==markers
        print('PASS slow-reader hook ignored without marked-fixture configuration')
    finally:
        (f.root/'rust-fault.json').unlink(missing_ok=True)
        for client in clients:client.close()
        f.stop()

if __name__=='__main__':run()
