#!/usr/bin/env python3
"""Provision the NFS service identity on managed directories, never on existing files.

Run on the NAS. Dry-run is the default; --apply requires an external backup directory.
ACL masks are expanded only after preserving all other principals' effective rights.
Restore with: setfacl --restore=<backup>/before.acl
"""
import argparse,json,os,stat,subprocess
from pathlib import Path

ROOTS=('creators','My Telegram','.face-cleanup-staging')

def command(*args):return subprocess.check_output(args,text=True)
def bits(s):return sum(n for c,n in zip(s,(4,2,1)) if c!='-')
def perms(n):return ''.join(c if n&v else '-' for c,v in zip('rwx',(4,2,1)))
def acl_changes(raw,uid):
 entries={}
 for line in raw.splitlines():
  line=line.split('#',1)[0].strip()
  if not line:continue
  key,value=line.rsplit(':',1);entries[key]=value
 desired={}
 for prefix in ('','default:'):
  masked=bits(entries.get(prefix+'mask:', 'rwx'))
  for key,value in entries.items():
   if key.startswith('default:')!=(prefix=='default:'):continue
   local=key[len(prefix):]
   if local=='group:' or local.startswith('group:') or (local.startswith('user:') and local!='user:'):
    if local!='user:'+str(uid):desired[key]=perms(bits(value)&masked)
  for field in ('user:','group:','other:'):
   if prefix and prefix+field not in entries:desired[prefix+field]=perms(bits(entries[field])&bits(entries.get('mask:','rwx'))) if field=='group:' else entries[field]
  desired[prefix+'user:'+str(uid)]='rwx'
  desired[prefix+'mask:']='rwx'
 return {k:v for k,v in desired.items() if entries.get(k)!=v}

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--base',type=Path,required=True);parser.add_argument('--uid',type=int,default=1000);parser.add_argument('--backup',type=Path);parser.add_argument('--apply',action='store_true');a=parser.parse_args()
 base=a.base.resolve();assert base.name=='pikpak' and a.uid>0
 if a.apply:assert a.backup and not a.backup.resolve().is_relative_to(base),'Backup must be outside the media tree'
 paths=[]
 for name in ROOTS:
  root=base/name
  if not root.is_dir() or root.is_symlink():raise RuntimeError('Managed root missing or symlink: '+str(root))
  for directory,dirs,files in os.walk(root,followlinks=False):
   dirs[:]=[d for d in dirs if not (Path(directory)/d).is_symlink()]
   paths.append(directory)
 records=[]
 for p in paths:
  s=os.lstat(p);raw=command('getfacl','-pn',p);changes=acl_changes(raw,a.uid)
  if changes:records.append({'path':p,'ino':s.st_ino,'mode':s.st_mode,'uid':s.st_uid,'gid':s.st_gid,'acl':raw,'changes':changes})
 print(json.dumps({'directories':len(paths),'to_update':len(records),'apply':a.apply}),flush=True)
 if not a.apply:return
 a.backup.mkdir(mode=0o700,parents=True,exist_ok=False)
 (a.backup/'before.acl').write_text('\n'.join(r['acl'] for r in records))
 (a.backup/'scope.json').write_text(json.dumps(records,ensure_ascii=False,indent=2))
 changed=[]
 try:
  for r in records:
   p=r['path'];s=os.lstat(p)
   if s.st_ino!=r['ino'] or command('getfacl','-pn',p)!=r['acl']:raise RuntimeError('Directory changed during provisioning: '+p)
   specs=','.join(k+':'+v for k,v in r['changes'].items())
   subprocess.run(['setfacl','-m',specs,p],check=True);changed.append(r)
   s=os.lstat(p)
   if (s.st_uid,s.st_gid)!=(r['uid'],r['gid']):raise RuntimeError('Unexpected ownership change')
   if acl_changes(command('getfacl','-pn',p),a.uid):raise RuntimeError('ACL verification failed: '+p)
 except BaseException:
  subprocess.run(['setfacl','--restore=-'],input='\n'.join(r['acl'] for r in changed),text=True,check=True)
  raise
 (a.backup/'receipt.json').write_text(json.dumps({'updated':len(changed),'uid':a.uid,'existing_files_modified':False}))
 print(json.dumps({'updated':len(changed),'backup':str(a.backup),'existing_files_modified':False}),flush=True)

if __name__=='__main__':main()
