"""Scheduled, read-only media inspection. Only publishes reversible UI visibility metadata."""
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

def publish(path, value):
    fd, tmp = tempfile.mkstemp(prefix='.'+path.name+'-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as out:
            json.dump(value, out, ensure_ascii=False); out.flush(); os.fsync(out.fileno())
        os.chmod(tmp, 0o644); os.replace(tmp, path)
    finally:
        if os.path.exists(tmp): os.unlink(tmp)

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

def run(base, root, expected_source, budget=300, fixture=False, now=None, seconds=1200):
    now=now or dt.datetime.now(UTC)
    deadline=time.monotonic()+seconds
    target=base/'video-health.json'; previous=read_json(target,{'schema':1,'records':{}})
    summary={**previous, 'schema':1, 'started_at':timestamp(now), 'completed_at':None, 'next_run_at':next_run(now),
             'schedule':'每天 03:30（北京时间）', 'status':'running', 'policy':dict(checks=3, minimum_hours=40, delete_files=False)}
    publish(target,summary)
    try:
        check_mount(root,expected_source,fixture)
        sources, review, archived, ids=inventory(base)
        reports=read_json(base/'video-health-reports.json',{})
        current={vid:resolve(vid,sources.get(vid),archived.get(vid,{}).get('win_dst'),root) for vid in sorted(ids)}
        missing=sum(r['status'] in DEFINITIVE for r in current.values())
        if ids and missing > max(50,len(ids)*.2): raise RuntimeError('异常数量突然偏多，本轮只记录检查失败，不自动收起')
        old_records=previous.get('records',{}); records={}; candidates=[]
        for vid, item in current.items():
            before=old_records.get(vid,{})
            same=all(before.get(k)==item.get(k) for k in ['path','size','mtime_ns'])
            item.update(vid=vid,in_review=vid in review)
            if item['status']=='present':
                report=reports.get(vid,{})
                reported=report.get('last_reported_at','') if not report.get('path') or report['path']==item['path'] else ''
                fresh_report=parse_time(reported)>parse_time(before.get('last_probed_at'))
                due=not same or before.get('status') in DEFINITIVE|{'unavailable','unmapped'} or (now-parse_time(before.get('last_probed_at'))).days>=30 or fresh_report
                if due:
                    # User-reported failures first, then current review videos, then older media.
                    active=vid in review and not item['path'].startswith(str(root)+'/pikpak/creators/')
                    candidates.append((0 if fresh_report else 1 if before.get('status') in DEFINITIVE else 2 if active else 3, before.get('last_probed_at',''),vid))
                    if same and before.get('last_probed_at'):
                        records[vid]={**before,'in_review':vid in review}
                    else: records[vid]={**item,'status':'unchecked','hidden':False,'consecutive':0}
                else: records[vid]={**before,'in_review':vid in review}
            else: records[vid]=transition(before,item,now)
        probed=0
        for _,_,vid in sorted(candidates)[:budget]:
            if time.monotonic()>=deadline: break
            original=current[vid]; before=old_records.get(vid,{})
            result=probe(original['path'])
            # A concurrent archive/rename invalidates the probe; never quarantine that stale path.
            after=resolve(vid,sources.get(vid),archived.get(vid,{}).get('win_dst'),root)
            if any(original.get(k)!=after.get(k) for k in ['path','size','mtime_ns']):
                result=dict(status='unavailable',detail='检查期间路径或文件发生变化，等待下次复查')
            report=reports.get(vid,{})
            if result['status']=='healthy' and report and (not report.get('path') or report['path']==original['path']) and (now-parse_time(report.get('last_reported_at'))).days<30:
                result.update(status='playback_warning',detail='曾报告播放失败，本次视频结构检查通过；需核对网络或手机兼容性')
            records[vid]=transition(before,{**original,**result,'last_probed_at':timestamp(now)},now)
            probed+=1
        check_mount(root,expected_source,fixture)
        summary.update(status='complete',records=records,last_success_at=timestamp(now),probed=probed,
            waiting=max(0,len(candidates)-probed),counts=dict(collections.Counter(r['status'] for r in records.values())),
            hidden=sum(bool(r.get('hidden')) for r in records.values()),total=len(records))
        summary.pop('error',None)
    except Exception as error:
        summary.update(status='skipped',error=str(error)[:500])
    summary['completed_at']=timestamp()
    publish(target,summary)
    print(json.dumps({k:v for k,v in summary.items() if k!='records'},ensure_ascii=False))
    return summary

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--base',type=Path,default=Path(os.environ.get('FACES_BASE','/data/base')))
    parser.add_argument('--root',type=Path,default=Path(os.environ.get('MEDIA2_ROOT','/mnt/media2')));parser.add_argument('--budget',type=int,default=300)
    parser.add_argument('--expected-source',default=os.environ.get('MEDIA_NFS_SOURCE',''));parser.add_argument('--fixture',action='store_true')
    parser.add_argument('--seconds',type=int,default=1200)
    args=parser.parse_args()
    if args.budget<0 or args.budget>1000: parser.error('budget must be 0..1000')
    with open(args.base/'.video-health.lock','w') as lock:
        try: fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError: raise SystemExit('video health scan already running')
        result=run(args.base,args.root,args.expected_source,args.budget,args.fixture,seconds=args.seconds)
    raise SystemExit(0 if result['status']=='complete' else 1)
