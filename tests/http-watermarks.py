"""HTTP contract tests against the disposable fixture on port 8922 only."""
import datetime as dt
import hashlib
import json
from pathlib import Path
import sqlite3
import urllib.error
import urllib.request

BASE = 'http://127.0.0.1:8922'
ROOT = Path('/tmp/face-admin-v10-fixture')
assert (ROOT / '.fixture-marker').exists()
STORE = ROOT / 'base/watermarks'
STORE.mkdir(exist_ok=True)


def request(route, body=None, method=None, expected=200, raw=False):
    req = urllib.request.Request(BASE + route, data=json.dumps(body).encode() if body is not None else None,
                                 headers={'Content-Type': 'application/json'}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            code, data = response.status, response.read()
    except urllib.error.HTTPError as error:
        code, data = error.code, error.read()
    assert code == expected, (route, code, data)
    return data if raw else json.loads(data)


def write(name, data):
    (STORE / name).write_text(json.dumps(data, ensure_ascii=False))


def vid(stem):
    return 'R:\\media\\pikpak\\My Telegram\\test\\' + stem + '.mp4'


file = ROOT / 'media/pikpak/My Telegram/test/watermark-no-face.mp4'
file.write_bytes((file.parent / 'a.mp4').read_bytes())
con = sqlite3.connect(ROOT / 'base/faces.db')
con.execute('INSERT OR REPLACE INTO videos VALUES (?, ?)', (vid('watermark-no-face'), str(file)))
con.commit()
con.close()
(STORE / 'worker.json').unlink(missing_ok=True)
assert request('/api/watermarks')['worker']['online'] is False
request('/api/watermarks', {'vids': [vid('watermark-no-face')]}, expected=503)
write('worker.json', {'at': dt.datetime.now(dt.timezone.utc).isoformat(), 'pipeline': 'test-v1'})
request('/api/watermarks', {'vids': [str(i) for i in range(101)]}, expected=400)
request('/api/watermarks', {'vids': [vid('keep')]}, expected=409)
job = request('/api/watermarks', {'vids': [vid('watermark-no-face'), vid('a')]}, expected=202)
assert job['total'] == 2
assert set(request('/api/watermarks')['job']['vids']) == {vid('watermark-no-face'), vid('a')}
request('/api/watermarks', {'vids': [vid('watermark-no-face')]}, expected=409)

key = hashlib.sha256(b'unknown\0@fixture_account').hexdigest()
(STORE / 'evidence').mkdir(exist_ok=True)
image = STORE / 'evidence' / (key + '.jpg')
image.write_bytes(b'fixture evidence')
candidate = dict(key=key, text='@fixture_account', token='@fixture_account', platform='unknown', kind='account',
                 frames=3, score=.99, timestamp=1.2, evidence=image.name)
records = {}
for stem in ['a', 'watermark-no-face']:
    media = file.parent / (stem + '.mp4')
    stat = media.stat()
    records[vid(stem)] = dict(vid=vid(stem), path=str(media), size=stat.st_size, mtime_ns=str(stat.st_mtime_ns),
                             pipeline='test-v1', job_id=job['id'], status='done', checked_at='fixture',
                             sampled_frames=8, candidates=[candidate])
write('results.json', records)
write('progress.json', dict(job_id=job['id'], status='complete', processed=2, total=2, failed=0))

def row(stem):
    return next(v for v in request('/api/watermarks')['videos'] if v['vid'] == vid(stem))

assert row('watermark-no-face')['suggested_creator'] is None
request('/api/watermarks', dict(key=key, kind='source'), method='PUT')
assert row('watermark-no-face')['suggested_creator'] is None
request('/api/watermarks', dict(key=key, kind='person', creator='水印测试人物'), method='PUT')
assert row('watermark-no-face')['suggested_creator'] == '水印测试人物'
assert row('a')['conflict'] is True
assert row('a')['suggested_creator'] is None
from urllib.parse import urlencode
evidence_url = '/api/watermarks/evidence?' + urlencode(dict(vid=vid('watermark-no-face'), key=key))
assert request(evidence_url, raw=True) == b'fixture evidence'
request('/api/watermarks/evidence?' + urlencode(dict(vid=vid('watermark-no-face'), key='../results.json')), expected=404, raw=True)

request('/api/plan/queue', dict(vids=[vid('watermark-no-face')], creator='水印测试人物', source='watermark', source_id=key))
assert vid('watermark-no-face') not in {v['vid'] for v in request('/api/watermarks')['videos']}
undo = request('/api/plan/queue', dict(vids=[vid('watermark-no-face')]), method='DELETE')
assert undo['removed'][0]['return_href'] == '/watermarks'
assert row('watermark-no-face')['suggested_creator'] == '水印测试人物'
file.write_bytes(file.read_bytes() + b'changed')
assert row('watermark-no-face')['watermark'] is None
request(evidence_url, expected=404, raw=True)
request('/api/watermarks', dict(key=key, kind='remove'), method='PUT')
assert key not in request('/api/watermarks')['mappings']
request('/api/watermarks', dict(key='../bad', kind='source'), method='PUT', expected=400)
assert file.exists()
print('PASS offline worker, job exclusion, batch limit, no-face OCR, mappings, face conflicts, evidence, queue/undo, replacement invalidation')
