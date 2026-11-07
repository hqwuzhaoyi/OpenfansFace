import importlib.util,unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('permissions',Path(__file__).parents[1]/'maintenance/media-directory-permissions.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class ACLTest(unittest.TestCase):
 def test_preserves_masked_principals_and_expands_only_service(self):
  raw='user::rwx\nuser:2000:rwx\ngroup::rwx\ngroup:3000:rw-\nmask::r-x\nother::---\n'
  c=module.acl_changes(raw,1000)
  self.assertEqual(c['user:2000'],'r-x');self.assertEqual(c['group:'],'r-x');self.assertEqual(c['group:3000'],'r--');self.assertEqual(c['user:1000'],'rwx')
 def test_default_inheritance_and_idempotence(self):
  raw='user::rwx\ngroup::r-x\nother::r-x\n'
  c=module.acl_changes(raw,1000);self.assertEqual(c['default:user:1000'],'rwx');self.assertEqual(c['default:group:'],'r-x')
  entries=dict(line.rsplit(':',1)for line in raw.splitlines());entries.update(c)
  self.assertEqual(module.acl_changes('\n'.join(k+':'+v for k,v in entries.items()),1000),{})
if __name__=='__main__':unittest.main()
