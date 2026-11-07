"""Marked scalar-only fixtures; the sentinel vector member must never be opened."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT=Path(__file__).parents[1]
def module(name,path):
    spec=importlib.util.spec_from_file_location(name,path);value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
a=module('audit',ROOT/'maintenance/legacy_reference_audit.py')
r=module('controller',ROOT/'maintenance/release_control.py')


def scalar_npy(value):
    if isinstance(value,int):descriptor='<i8';body=value.to_bytes(8,'little',signed=True)
    else:descriptor='<U'+str(len(value));body=value.encode('utf-32-le')
    header=repr({'descr':descriptor,'fortran_order':False,'shape':()}).encode()+b'\n'
    return b'\x93NUMPY\x01\x00'+len(header).to_bytes(2,'little')+header+body


def cache(path,source,version=a.VERSION):
    observed=source.stat()
    with zipfile.ZipFile(path,'w',compression=zipfile.ZIP_DEFLATED) as archive:
        for key,value in dict(version=version,path=str(source),size=observed.st_size,mtime_ns=str(observed.st_mtime_ns)).items():archive.writestr(key+'.npy',scalar_npy(value))
        archive.writestr('embeddings.npy',b'FORBIDDEN vector member, deliberately not an NPY')


class Fixture:
    def __init__(self,count=1):
        self.temp=tempfile.TemporaryDirectory(prefix='marked-legacy-reference-')
        self.root=Path(self.temp.name).resolve();self.base=self.root/'base';self.media=self.root/'media'
        (self.base/'processing/faces').mkdir(parents=True);(self.media/'pikpak/creators/Fixture Person').mkdir(parents=True)
        (self.base/'processing/references.npz').write_bytes(b'existing reference artifact must stay byte-identical')
        self.references=(self.base/'processing/references.npz').read_bytes();self.sources=[];self.ids=[]
        with sqlite3.connect(self.base/'faces.db') as db:
            db.execute('CREATE TABLE videos(vid TEXT PRIMARY KEY,path TEXT)')
            for index in range(count):
                vid='private-stable-id-'+str(index);source=self.media/'pikpak/creators/Fixture Person'/('video-'+str(index)+'.mp4');source.write_bytes(b'isolated bytes')
                self.ids.append(vid);self.sources.append(source);db.execute('INSERT INTO videos VALUES(?,?)',(vid,str(source)))
                cache(self.base/'processing/faces'/(hashlib.sha256(vid.encode()).hexdigest()+'.npz'),source)
        self.write('integration-queue.json',[{'vid':vid,'creator':'Fixture Person'} for vid in self.ids]);self.write('archive-history.json',[])
    def write(self,name,value):
        path=self.base/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(value))
    def close(self):self.temp.cleanup()
    def unmapped_old_archives(self):
        history=[]
        for vid,source in zip(self.ids,self.sources):
            target=source.with_name('old-archived-'+source.name);source.rename(target)
            history.append({'vid':vid,'creator':'Fixture Person','status':'ok','src':str(source),'dst':str(target),'manifest':'old-complete.json'})
        self.write('integration-queue.json',[]);self.write('archive-history.json',history)
        self.write('manifests/old-complete.json',{'status':'complete','moves':history})
        return history


class LegacyReferenceAuditTests(unittest.TestCase):
    def test_ten_old_path_only_inputs_block_before_service_mutation_and_preserve_references(self):
        f=Fixture(10)
        try:
            before=a.audit(f.base,f.media);self.assertEqual(before['status'],'passed')
            f.unmapped_old_archives()
            current=a.audit(f.base,f.media);self.assertEqual(current['status'],'failed');self.assertEqual(current['counts']['unresolved'],10)
            m={'base':str(f.base),'media_root':str(f.media),'native_url':'http://127.0.0.1:1','native_token_env':'UNSET_ISOLATED_TOKEN'}
            with patch.object(r,'run') as mutate:
                with self.assertRaisesRegex(RuntimeError,'release gate blocked'):r.legacy_reference_gate(m,before)
                mutate.assert_not_called()
            self.assertEqual((f.base/'processing/references.npz').read_bytes(),f.references)
            serialized=json.dumps(current);self.assertNotIn(str(f.root),serialized);self.assertNotIn('Fixture Person',serialized);self.assertNotIn(f.ids[0],serialized)
        finally:f.close()

    def test_only_whitelisted_scalar_members_are_opened_and_current_inputs_pass(self):
        f=Fixture();opened=[];original=zipfile.ZipFile.open
        try:
            def guard(archive,key,*args,**kwargs):
                name=key.filename if isinstance(key,zipfile.ZipInfo) else key
                self.assertIn(name,a.SCALARS);opened.append(name);return original(archive,key,*args,**kwargs)
            with patch.object(zipfile.ZipFile,'open',guard):result=a.audit(f.base,f.media)
            self.assertEqual(result['status'],'passed');self.assertEqual(result['counts']['direct'],1);self.assertEqual(len(opened),4)
            with self.assertRaisesRegex(ValueError,'forbidden'):a.scalar(None,'embeddings.npy')
            m={'base':str(f.base),'media_root':str(f.media),'native_url':'http://127.0.0.1:1','native_token_env':'UNSET_ISOLATED_TOKEN'}
            self.assertEqual(r.legacy_reference_gate(m,result)['input_version'],result['input_version'])
        finally:f.close()

    def test_malformed_cache_source_database_or_inventory_is_unknown_never_zero(self):
        for corruption in ('zip','json','database','symlink'):
            with self.subTest(corruption=corruption):
                f=Fixture()
                try:
                    if corruption=='zip':next((f.base/'processing/faces').glob('*.npz')).write_bytes(b'bad ZIP')
                    elif corruption=='json':(f.base/'integration-queue.json').write_text('{bad')
                    elif corruption=='database':(f.base/'faces.db').write_bytes(b'not sqlite')
                    else:
                        directory=f.base/'processing/faces';directory.rename(f.base/'processing/real-faces');directory.symlink_to(f.base/'processing/real-faces')
                    result=a.audit(f.base,f.media);self.assertEqual(result['status'],'unknown')
                    self.assertTrue(result.get('counts') is None or result['counts']['unknown']>0)
                    self.assertEqual((f.base/'processing/references.npz').read_bytes(),f.references)
                finally:f.close()

    def test_hashed_passed_flag_cannot_override_failed_unknown_or_changed_audit(self):
        f=Fixture()
        try:
            m={'base':str(f.base),'media_root':str(f.media),'native_url':'http://127.0.0.1:1','native_token_env':'UNSET_ISOLATED_TOKEN'}
            proof=a.audit(f.base,f.media)
            for status in ('failed','unknown'):
                with self.assertRaisesRegex(RuntimeError,'unresolved'):r.legacy_reference_gate(m,{**proof,'status':status,'passed':True})
            f.write('person-renames.json',{'Fixture Person':'Renamed'})
            with self.assertRaisesRegex(RuntimeError,'inputs changed'):r.legacy_reference_gate(m,proof)
            with self.assertRaisesRegex(RuntimeError,'simulated'):r.legacy_reference_gate(m,{**proof,'simulated':True,'gate':'legacy_reference_compatibility'})
        finally:f.close()

    def test_required_gate_is_distinct_from_rename_parity_and_fails_before_pause(self):
        m=json.loads((ROOT/'deployment/native-release.manifest.example.json').read_text())
        with tempfile.TemporaryDirectory(prefix='marked-gate-evidence-') as tmp:
            proof=Path(tmp)/'rename-parity.json';proof.write_text('{"passed":true,"rename_files":2}')
            evidence={'passed':True,'path':str(proof),'sha256':hashlib.sha256(proof.read_bytes()).hexdigest()}
            m['gpu_peer']['prepared_receipt']=evidence;m['evidence']={gate:evidence.copy() for gate in r.GATES}
            with patch.object(r,'run') as mutate,patch.object(r,'wait_peer_idle') as pause:
                with self.assertRaisesRegex(RuntimeError,'legacy_reference_compatibility'):r.fence_peers(m,0)
                mutate.assert_not_called();pause.assert_not_called()
            del m['evidence']['legacy_reference_compatibility']
            with self.assertRaisesRegex(RuntimeError,'legacy_reference_compatibility'):r.validate(m,release=True)

if __name__=='__main__':unittest.main()
