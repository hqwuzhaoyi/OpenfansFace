"""Read-only historical policy helpers. Publishing and the old timer are retired."""
import argparse, collections, datetime as dt, fcntl, json, os, sqlite3, stat, subprocess, tempfile, time
from pathlib import Path
from zoneinfo import ZoneInfo

DEFINITIVE = {'missing', 'not_file', 'empty', 'corrupt'}
UTC = dt.timezone.utc

def timestamp(now=None):
    return (now or dt.datetime.now(UTC)).isoformat()

def parse_time(value):
    try: return dt.datetime.fromisoformat(value)
    except (ValueError, TypeError): return dt.datetime.min.replace(tzinfo=UTC)

def read_json(path, fallback):
    try: return json.loads(path.read_text())
    except FileNotFoundError: return fallback

def publish(*args, **kwargs):
    raise RuntimeError('Native Rust owns video health; legacy publication is disabled')


def next_run(now):
    local = now.astimezone(ZoneInfo('Asia/Shanghai'))
    target = local.replace(hour=3, minute=30, second=0, microsecond=0)
    if target <= local: target += dt.timedelta(days=1)
    return target.isoformat()

def mapped(value, root):
    if not isinstance(value, str): return None
    value = value.replace('\\', '/')
    if '..' in value.split('/'): return None
    if value.startswith(str(root)+'/'): return value
    marker = '/media/pikpak/'
    return str(root)+'/pikpak/'+value.split(marker, 1)[1] if marker in value else None

def resolve(vid, stored, archived, root):
    candidates = list(dict.fromkeys(p for p in [mapped(archived,root),mapped(stored,root),mapped(vid,root)] if p))
    dirs, errors = [], []
    for name in candidates:
        try:
            info = os.stat(name)
            if stat.S_ISREG(info.st_mode):
                return dict(path=name, size=info.st_size, mtime_ns=str(info.st_mtime_ns), mtime_ms=info.st_mtime_ns//1_000_000,
                            status='present' if info.st_size else 'empty')
            dirs.append(name)
        except (FileNotFoundError, NotADirectoryError): pass
        except OSError as error: errors.append(str(error))
    return dict(path=candidates[0] if candidates else None, size=None, mtime_ns=None, mtime_ms=None,
                status='unavailable' if errors else 'not_file' if dirs else 'missing' if candidates else 'unmapped',
                detail=errors[0] if errors else '')

def probe(path, timeout=20):
    try:
        result = subprocess.run(['ffprobe','-v','error','-probesize','5000000','-analyzeduration','5000000',
            '-show_entries','stream=codec_type,codec_name,pix_fmt:format=format_name,duration','-of','json',path],
            capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired: return dict(status='unavailable', detail='读取超时，下次再检查')
    except OSError as error: return dict(status='unavailable', detail=str(error)[:300])
    try: data = json.loads(result.stdout)
    except ValueError: data = {}
    streams = [s for s in data.get('streams',[]) if s.get('codec_type')=='video']
    error = result.stderr.lower()
    if result.returncode:
        if any(s in error for s in ['moov atom not found','invalid data found when processing input','end of file']):
            return dict(status='corrupt', detail='无法读取有效视频结构：'+result.stderr.strip()[:200])
        return dict(status='unavailable', detail='暂时无法读取：'+result.stderr.strip()[:200])
    if not streams: return dict(status='corrupt', detail='文件中没有可识别的视频轨道')
    codec = streams[0].get('codec_name','unknown'); pixel = streams[0].get('pix_fmt','')
    incompatible = codec not in {'h264','hevc','vp8','vp9','av1'} or (codec=='h264' and pixel not in {'yuv420p','yuvj420p','nv12'})
    return dict(status='compatibility' if incompatible else 'healthy', codec=codec, pixel_format=pixel,
                detail='视频结构正常，手机编码兼容性需核对' if incompatible else '视频结构检查通过')

def transition(previous, observed, now):
    record = {**observed, 'last_checked_at':timestamp(now), 'hidden':False, 'consecutive':0}
    if observed['status'] not in DEFINITIVE: return record
    unchanged = all(previous.get(k)==observed.get(k) for k in ['path','size','mtime_ns','status'])
    if unchanged:
        record['consecutive'] = previous.get('consecutive',0)
        record['first_failure_at'] = previous.get('first_failure_at',timestamp(now))
        record['last_counted_at'] = previous.get('last_counted_at',record['first_failure_at'])
        if (now-parse_time(record['last_counted_at'])).total_seconds() >= 20*3600:
            record['consecutive'] += 1; record['last_counted_at'] = timestamp(now)
    else:
        record.update(consecutive=1, first_failure_at=timestamp(now), last_counted_at=timestamp(now))
    record['hidden'] = record['consecutive']>=3 and (now-parse_time(record['first_failure_at'])).total_seconds()>=40*3600
    return record

def check_mount(root, expected_source, fixture=False):
    if fixture:
        if not str(root).startswith('/tmp/') or not (root.parent/'.fixture-marker').exists():
            raise RuntimeError('Local fixture requires /tmp root and .fixture-marker')
    else:
        if not expected_source: raise RuntimeError('请配置 MEDIA_NFS_SOURCE 或 --expected-source，指定预期的网络盘来源')
        actual = subprocess.check_output(['findmnt','-n','-T',str(root),'-o','FSTYPE,SOURCE'],text=True,timeout=5).split()
        if len(actual)!=2 or actual[0] not in {'nfs','nfs4'} or actual[1]!=expected_source:
            raise RuntimeError('网络盘挂载与预期不符，本轮不收起任何记录')
    for folder in [root/'pikpak/creators',root/'pikpak/My Telegram']:
        if not folder.is_dir() or not next(folder.iterdir(),None):
            raise RuntimeError('媒体目录不可用或为空，本轮不收起任何记录')

def inventory(base):
    con=sqlite3.connect('file:'+str(base/'faces.db')+'?mode=ro',uri=True)
    try: sources=dict(con.execute('SELECT vid,path FROM videos'))
    finally: con.close()
    report=read_json(base/'report.json',{}); clusters=read_json(base/'clusters.json',[])
    claims=read_json(base/'claims-detail.json',[]); queue=read_json(base/'integration-queue.json',[])
    history=read_json(base/'archive-history.json',[])
    review={v['vid'] for person in report.get('identities',[]) for v in person.get('vids',[])}
    review.update(v['vid'] for c in clusters for v in c.get('vids',[])); review.update(v['vid'] for v in claims)
    archived={v['vid']:v for v in history}
    ids=set(sources)|review|{v['vid'] for v in queue+history}
    return sources, review, archived, ids

def run(*args, **kwargs):
    raise RuntimeError('Native Rust owns video health; legacy publication is disabled')


if __name__=='__main__':
    raise SystemExit('Native Rust owns video health. Disable the legacy timer; use VIDEO_HEALTH_ENABLED=1 on the Rust service.')
