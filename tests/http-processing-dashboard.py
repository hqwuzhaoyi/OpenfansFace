"""Read and settings checks against an explicitly marked isolated #19 web fixture."""
import json, os, urllib.error, urllib.request
from pathlib import Path
root=Path(os.environ.get('PROCESSING_DASHBOARD_FIXTURE','/tmp/openfansface-rust-implementation/worker19-fixture'))
assert root.name=='worker19-fixture' and (root/'.fixture-marker').read_text()=='worker19-isolated'
url=os.environ.get('PROCESSING_DASHBOARD_URL','http://127.0.0.1:8969')
assert url.startswith('http://127.0.0.1:')
def request(route,body=None,method=None,expected=200):
    req=urllib.request.Request(url+route,data=None if body is None else json.dumps(body).encode(),method=method,headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=20) as response:status,data=response.status,json.load(response)
    except urllib.error.HTTPError as error:status,data=error.code,json.load(error)
    assert status==expected,(route,status,data)
    return data
before=request('/api/processing/dashboard')
assert before['preview'] and before['preview_id']=='worker19-isolated'
assert before['ledger_ready'] and before['totals']['total']==8 and before['totals']['completed']==3
assert sum(before['totals'][key] for key in ['completed','running','waiting','error','excluded'])==8
assert before['current']['running']==2
ids={job['id'] for job in before['jobs']};assert len(ids)==2
assert before['inference']['backend']=='gpu-detection' and before['inference']['available'] is False
request('/api/processing/settings',{'concurrency':1},'PUT')
after=request('/api/processing/dashboard')
assert after['settings']['effective']==1 and {job['id'] for job in after['jobs']}==ids
assert after['totals']==before['totals']
for invalid in [0,3,'2',True,1.5,None]:request('/api/processing/settings',{'concurrency':invalid},'PUT',400)
request('/api/processing/settings',{'concurrency':2},'PUT')
assert len([job for job in request('/api/home/tasks')['jobs'] if job['status']=='running'])==2
print('PASS marked dashboard API, fixed counts, true unavailable provenance, settings persistence/validation, two-to-one draining, distinct progress')
