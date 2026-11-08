"""Exact, metadata-only release decisions on marked synthetic legacy inputs."""
import copy
import hashlib
import json
from pathlib import Path
import sqlite3
import sys
import unittest
from unittest.mock import patch

from test_legacy_reference_audit import Fixture, a, r, cache, ROOT


def reference(path):
    return {'path':str(path), 'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}


class LegacyReferenceExclusionTests(unittest.TestCase):
    def setUp(self):
        self.f = Fixture(10)
        self.history = self.f.unmapped_old_archives()
        self.report = a.audit(self.f.base, self.f.media)
        self.authorization = self.f.root / 'user-authorization.json'
        self.authorization.write_text('{"fixture":true,"decision":"exclude these ten unsupported reference inputs; retain bytes"}')
        self.decision = {
            'version':1, 'purpose':'exclude_from_reference_build', 'release_id':'marked-test-release',
            'reason':'Explicit marked fixture authorization for ten unsupported legacy inputs',
            'reviewed_by':'fixture operator', 'reviewed_at':'2026-01-01T00:00:00+00:00',
            'authorization_reference':reference(self.authorization), 'preserve_original_bytes':True,
            'worker_policy':'source_unresolved_skipped_before_cache_read_v1',
            'reviewed_cases':copy.deepcopy(self.report['cases'])}
        self.manifest = {'release_id':'marked-test-release', 'base':str(self.f.base),
            'media_root':str(self.f.media), 'native_url':'http://127.0.0.1:1', 'native_token_env':'UNSET_ISOLATED_TOKEN'}
        self.freeze()

    def tearDown(self):
        self.f.close()

    def freeze(self):
        path = self.f.root / 'decision.json';path.write_text(json.dumps(self.decision))
        self.manifest['legacy_reference_exclusion'] = reference(path)

    def gate(self, proof=None):
        with patch.object(r, 'run') as mutation:
            try:
                return r.legacy_reference_gate(self.manifest, proof or self.report)
            finally:
                mutation.assert_not_called()

    def test_exact_ten_pass_with_truthful_counts_and_original_bytes_untouched(self):
        paths = list((self.f.base/'processing/faces').iterdir()) + [Path(row['dst']) for row in self.history] + [self.f.base/'processing/references.npz']
        before = {str(path):path.read_bytes() for path in paths}
        result = self.gate()
        self.assertEqual(result['status'], 'passed_with_approved_exclusions')
        self.assertEqual(result['counts']['unresolved'],10)
        self.assertEqual(result['counts']['approved_excluded'],10)
        self.assertEqual(result['counts']['native_proven'],0)
        self.assertEqual(result['numerical_reference_effect'],'not_evaluated')
        self.assertEqual(before,{str(path):path.read_bytes() for path in paths})
        self.assertEqual(self.report['status'],'failed')
        del self.manifest['legacy_reference_exclusion']
        with self.assertRaisesRegex(RuntimeError,'unresolved'):self.gate()

    def test_decision_is_structured_hash_bound_and_release_specific(self):
        for change in ('generic','purpose','release','future','duplicate','fingerprint','authorization','sha'):
            with self.subTest(change=change):
                original=copy.deepcopy(self.decision)
                if change=='generic':self.decision={'passed':True}
                elif change=='purpose':self.decision['purpose']='origin_proven'
                elif change=='release':self.decision['release_id']='different-release'
                elif change=='future':self.decision['reviewed_at']='2999-01-01T00:00:00Z'
                elif change=='duplicate':self.decision['reviewed_cases'][0]=self.decision['reviewed_cases'][1]
                elif change=='fingerprint':self.decision['reviewed_cases'][0]['cache_fingerprint']='0'*64
                elif change=='authorization':self.decision['authorization_reference']['sha256']='0'*64
                self.freeze()
                if change=='sha':self.manifest['legacy_reference_exclusion']['sha256']='0'*64
                with self.assertRaises(RuntimeError):self.gate()
                self.decision=original;self.freeze()

    def test_changed_assignment_cache_source_and_resolved_inputs_do_not_inherit(self):
        for change in ('assignment','alias','cache','source','resolved','unreadable','new_unknown'):
            with self.subTest(change=change):
                f=Fixture(10)
                try:
                    history=f.unmapped_old_archives()
                    old_manifest,old_report,old_decision=self.manifest,self.report,self.decision
                    self.manifest={**self.manifest,'base':str(f.base),'media_root':str(f.media)}
                    self.report=a.audit(f.base,f.media)
                    self.decision={**self.decision,'reviewed_cases':copy.deepcopy(self.report['cases'][:10])}
                    self.freeze()
                    if change=='assignment':history[0]['creator']='Other';f.write('archive-history.json',history)
                    elif change=='alias':f.write('person-renames.json',{'Fixture Person':'Other'})
                    elif change=='cache':cache(next((f.base/'processing/faces').iterdir()),Path(history[0]['dst']),version='changed')
                    elif change=='source':f.write('video-path-overrides.json',{f.ids[0]:str(f.media/'other-missing.mp4')})
                    elif change=='resolved':Path(history[0]['dst']).rename(Path(history[0]['src']))
                    elif change=='unreadable':next((f.base/'processing/faces').iterdir()).write_bytes(b'invalid')
                    elif change=='new_unknown':
                        new_vid='new-unapproved-input'
                        history.append({'vid':new_vid,'creator':'Fixture Person'});f.write('archive-history.json',history)
                        (f.base/'processing/faces'/(hashlib.sha256(new_vid.encode()).hexdigest()+'.npz')).write_bytes(b'invalid')
                    with self.assertRaises(RuntimeError):self.gate()
                finally:
                    f.close();self.manifest,self.report,self.decision=old_manifest,old_report,old_decision;self.freeze()

    def test_even_reviewed_read_errors_and_consumable_paths_are_not_excludable(self):
        for reason in ('source_fingerprint','native_origin_unproven','model_metadata','PermissionError'):
            report=copy.deepcopy(self.report);report['cases'][0]['reason']=reason
            self.decision['reviewed_cases']=copy.deepcopy(report['cases']);self.freeze()
            with self.assertRaisesRegex(RuntimeError,'consumable or unknown'):self.gate(report)

    def test_unrelated_progress_and_unassigned_cache_arrival_do_not_stale_review(self):
        with sqlite3.connect(self.f.base/'faces.db') as db:
            db.execute('CREATE TABLE unrelated_progress(value INTEGER)')
            db.execute('INSERT INTO unrelated_progress VALUES(1)')
            db.execute('INSERT INTO videos VALUES(?,?)',('unassigned-progress',str(self.f.media/'inbox/missing.mp4')))
        (self.f.base/'processing/faces/unassigned.npz').write_bytes(b'not read')
        self.assertEqual(self.gate()['status'],'passed_with_approved_exclusions')

    def test_existing_reference_artifact_change_requires_fresh_report_not_new_exclusions(self):
        (self.f.base/'processing/references.npz').write_bytes(b'changed isolated reference sentinel')
        with self.assertRaisesRegex(RuntimeError,'inputs changed'):self.gate()
        fresh=a.audit(self.f.base,self.f.media)
        self.assertEqual(fresh['cases'],self.report['cases'])
        self.assertEqual(self.gate(fresh)['status'],'passed_with_approved_exclusions')

    def test_read_failure_or_generic_native_proof_cannot_clear_new_source(self):
        self.f.write('video-path-overrides.json',{self.f.ids[0]:self.history[0]['dst']})
        def unreadable(*args):raise PermissionError('isolated fixture read failure')
        for prover in (unreadable, lambda *args:{'verified':True,'passed':True}):
            fresh=a.audit(self.f.base,self.f.media,prover)
            self.assertNotEqual(fresh['status'],'passed')
            self.assertGreater(fresh['counts']['unknown']+fresh['counts']['unresolved'],0)
            with self.assertRaises(RuntimeError):self.gate(fresh)

    def test_inventory_defaults_cover_1605_without_sampling_and_limits_are_frozen(self):
        for index in range(1595):(self.f.base/f'processing/faces/unassigned-{index}.npz').write_bytes(b'not read')
        current=a.audit(self.f.base,self.f.media)
        self.assertEqual(current['inventory']['cache_files'],1605)
        self.assertEqual(current['counts']['assigned_caches'],10)
        self.assertEqual(current['input_version'],self.report['input_version'])
        small={'cache_files':1500,'directory_entries':5000}
        self.assertIsNone(a.audit(self.f.base,self.f.media,inventory=small)['counts'])
        self.manifest['legacy_reference_inventory_limits']=small
        with self.assertRaisesRegex(RuntimeError,'limits changed'):self.gate()
        for bad in ({'cache_files':0,'directory_entries':100}, {'cache_files':20001,'directory_entries':40000}, {'cache_files':True,'directory_entries':100}, {'wall_seconds':900}, {'cache_files':5000,'directory_entries':4999}):
            with self.subTest(bad=bad):self.assertFalse(a.audit(self.f.base,self.f.media,inventory=bad)['complete'])
        self.assertEqual(a.LIMITS['wall_seconds'],90)
        self.assertEqual(a.LIMITS['stat_calls'],5000)

    def test_normal_business_snapshot_keeps_reference_and_cache_for_recovery(self):
        out=self.f.root/'business-snapshot';r.snapshot_tree(self.f.base,out)
        self.assertEqual((out/'processing/references.npz').read_bytes(),self.f.references)
        for path in (self.f.base/'processing/faces').iterdir():
            self.assertEqual((out/'processing/faces'/path.name).read_bytes(),path.read_bytes())

    def test_worker_actually_skips_reviewed_inputs_before_cache_or_feature_access(self):
        # Execute the production builder using an empty scalar-only faces schema.
        # No vectors or models are present, loaded, or evaluated in this fixture.
        sys.path.insert(0,str(ROOT/'maintenance'))
        import processing_faces
        with sqlite3.connect(self.f.base/'faces.db') as db:db.execute('CREATE TABLE faces(vid TEXT, score REAL)')
        with patch.object(processing_faces,'native',return_value={'version':'fixture'}), patch.object(processing_faces,'reference_features') as features, patch.object(processing_faces,'save_npz') as save:
            processing_faces.build_references(self.f.base,self.f.media)
            features.assert_not_called();save.assert_called_once()
        self.assertEqual((self.f.base/'processing/references.npz').read_bytes(),self.f.references)

if __name__=='__main__':unittest.main()
