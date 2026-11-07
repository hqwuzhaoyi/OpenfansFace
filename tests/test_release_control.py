import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('release',Path(__file__).parents[1]/'maintenance/release_control.py')
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)

class ReleaseControlTests(unittest.TestCase):
    def manifest(self):
        return json.loads((Path(__file__).parents[1]/'deployment/native-release.manifest.example.json').read_text())

    def test_examples_do_not_authorize_release(self):
        with self.assertRaisesRegex(RuntimeError,'receipt missing'):
            r.validate(self.manifest(),release=True)

    def test_schema_downgrade_rejected(self):
        m=self.manifest();m['native_schema']=8
        with self.assertRaisesRegex(RuntimeError,'schema 9'):
            r.validate(m)

    def test_old_calibrator_is_not_drained_by_native_empty_queue(self):
        m=self.manifest()
        with patch.object(r,'inspect',return_value={'State':{'Running':True}}), self.assertRaisesRegex(RuntimeError,'old writer still running'):
            r.check_peer_drain(m)

    def test_container_change_fails_before_mutation(self):
        m=self.manifest()
        with patch.object(r,'inspect',return_value={'Id':'changed','Image':'changed'}),self.assertRaisesRegex(RuntimeError,'changed since inspection'):
            r.baseline(m)

    def test_snapshot_uses_backup_excludes_authority_and_cannot_reuse(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);base=root/'source';base.mkdir();(base/'.fixture').write_text('isolated')
            (base/'file-operations-rust').mkdir();(base/'file-operations-rust/executor.lock').write_text('not cloneable authority')
            (base/'manual.json').write_text('{"keep":true}')
            ledger=root/'ledger.db'
            db=sqlite3.connect(ledger);db.execute('PRAGMA journal_mode=WAL');db.execute('CREATE TABLE assets(id INTEGER)');db.execute('INSERT INTO assets VALUES(7)');db.commit()
            out=root/'snapshot';r.parity_snapshot(base,ledger,out)
            with sqlite3.connect(out/'ledger.db') as copied:self.assertEqual(copied.execute('SELECT * FROM assets').fetchall(),[(7,)])
            self.assertFalse((out/'base/file-operations-rust').exists())
            self.assertEqual((out/'base/manual.json').read_text(),'{"keep":true}')
            with self.assertRaises(FileExistsError):r.parity_snapshot(base,ledger,out)
            db.close()

    def test_snapshot_refuses_symlink_data(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);base=root/'source';base.mkdir();(base/'link').symlink_to('/does-not-matter')
            with self.assertRaisesRegex(RuntimeError,'symlink'):r.snapshot_tree(base,root/'snapshot')

    def test_capacity_blocker_does_not_prune_or_touch_containers(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);base=root/'base';base.mkdir();(base/'metadata.json').write_bytes(b'x'*100)
            ledger=root/'ledger.db';ledger.write_bytes(b'fixture')
            m=self.manifest();m.update(base=str(base),ledger=str(ledger),evidence_dir=str(root/'backup'))
            with patch.object(r.shutil,'disk_usage',return_value=SimpleNamespace(free=0)),patch.object(r,'run') as command,self.assertRaisesRegex(RuntimeError,'lacks space'):
                r.capacity(m)
            command.assert_not_called()

    def test_replace_records_partial_stop_before_failed_creation(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);receipt=root/'receipts';receipt.mkdir();m=self.manifest()
            original={role:{'Id':service['expected_id'],'Image':service['expected_image']} for role,service in m['services'].items()}
            calls=[]
            def command(*args):
                calls.append(args)
                if args[:2]==('docker','create'):raise RuntimeError('candidate creation failed')
                return ''
            with patch.object(r,'validate'),patch.object(r,'capacity'),patch.object(r,'baseline',return_value=original),patch.object(r,'gpu_probe'),patch.object(r,'legacy_health_gate'),patch.object(r,'legacy_journals'),patch.object(r,'check_peer_drain'),patch.object(r,'drain',return_value={'drained':True}),patch.object(r,'authority'),patch.object(r,'snapshot_tree'),patch.object(r,'backup_database'),patch.object(r,'inspect',return_value=original['native']),patch.object(r,'run',side_effect=command),self.assertRaisesRegex(RuntimeError,'creation failed'):
                r.replace(m,receipt)
            self.assertTrue((receipt/'native-stopped.json').exists())
            self.assertFalse((receipt/'native-started.json').exists())
            self.assertFalse((receipt/'worker-stopped.json').exists())
            self.assertFalse(any(args[:2]==('docker','start') for args in calls))

    def test_authority_matches_native_string_identity(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);lock=root/'executor.lock';lock.write_text('')
            stat=lock.stat();m={'authority_dir':str(root),'authority_identity':{'dev':str(stat.st_dev),'ino':str(stat.st_ino)}}
            self.assertEqual(r.authority(m),m['authority_identity'])

    def test_simulated_gate_cannot_be_used_as_production_evidence(self):
        import hashlib
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            proof=Path(tmp)/'SIMULATED.json';proof.write_text('{"simulated":true}')
            evidence={'passed':True,'path':str(proof),'sha256':hashlib.sha256(proof.read_bytes()).hexdigest()}
            m=self.manifest();m['gpu_peer']['prepared_receipt']=evidence;m['evidence']={g:evidence for g in r.GATES}
            with self.assertRaisesRegex(RuntimeError,'simulated gate evidence'):r.validate(m,release=True)

    def test_scratch_is_excluded_but_accepted_attempt_receipts_are_backed_up(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);base=root/'base';scratch=base/'processing/tmp';scratch.mkdir(parents=True)
            (scratch/'ephemeral').write_bytes(b'x'*1000)
            durable=base/'processing/attempts/accepted';durable.mkdir(parents=True);(durable/'publication.json').write_bytes(b'durable')
            self.assertEqual(r.snapshot_size(base),7)
            r.snapshot_tree(base,root/'backup')
            self.assertFalse((root/'backup/processing/tmp').exists())
            self.assertEqual((root/'backup/processing/attempts/accepted/publication.json').read_bytes(),b'durable')

    def test_transient_startup_retry_is_bounded_and_does_not_mask_bad_contract(self):
        import urllib.error
        with self.assertRaisesRegex(RuntimeError,'deadline exceeded'):
            r.await_available(lambda:(_ for _ in ()).throw(urllib.error.URLError('offline')),0)
        with self.assertRaisesRegex(RuntimeError,'wrong identity'):
            r.await_available(lambda:(_ for _ in ()).throw(RuntimeError('wrong identity')),30)

    def test_marked_rehearsal_guard_checks_actual_labels_and_mount_boundary(self):
        with tempfile.TemporaryDirectory(prefix='face-admin-release-rehearsal-',dir='/tmp') as tmp:
            root=Path(tmp).resolve();(root/'.release-rehearsal').write_text('isolated controller rehearsal')
            m=self.manifest();m['rehearsal']={'root':str(root),'id':'fixture'}
            for key in ('base','ledger','authority_dir','evidence_dir','deploy_lock'):m[key]=str(root/key)
            for role,service in m['services'].items():
                service['name']='face-release-rehearsal-fixture-'+role
                service['create_args']=['--network','face-release-rehearsal-fixture','--label','face-admin.release-rehearsal=fixture','-v',str(root)+':'+str(root)]
            m['peach']['name']='face-release-rehearsal-fixture-peach'
            with patch.dict('os.environ',{'FILE_OPERATIONS_RELEASE_REHEARSAL_ROOT':str(root)}),patch.object(r,'inspect',return_value={'Config':{'Labels':{'face-admin.release-rehearsal':'fixture'}}}):
                self.assertTrue(r.rehearsal_guard(m))
                m['services']['native']['create_args'][-1]='/outside:'+str(root)
                with self.assertRaisesRegex(RuntimeError,'mount escapes'):r.rehearsal_guard(m)

    def test_permission_roles_do_not_require_write_on_worker_artifacts_or_jobs(self):
        import os
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);base=root/'base';(base/'processing').mkdir(parents=True);(base/'jobs').mkdir()
            for path in (base/'faces.db',base/'processing/processing.db',root/'ledger.db'):path.write_bytes(b'fixture')
            m={'base':str(base),'ledger':str(root/'ledger.db'),'authority_dir':str(base/'file-operations-rust'),'native_uid':os.geteuid(),'native_gid':os.getegid()}
            result=r.permissions(m);rows={row.get('requested'):row for row in result['paths']}
            self.assertEqual(rows[str(base/'jobs')]['required'],'rx')
            self.assertEqual(rows[str(base/'processing/attempts')]['required'],'rx')
            self.assertEqual(rows[str(base/'processing')]['required'],'rwx')
            self.assertEqual(rows[str(base/'watermarks')]['required'],'rwx')

    def test_legacy_timer_must_be_disabled_but_inactive_static_service_is_valid(self):
        m=self.manifest()
        def show(*args):
            return 'LoadState=loaded\nActiveState=inactive\nUnitFileState='+('disabled' if args[2].endswith('.timer') else 'static')
        with patch.object(r,'run',side_effect=show):self.assertEqual(r.legacy_health_gate(m)['service']['UnitFileState'],'static')
        with patch.object(r,'run',return_value='LoadState=loaded\nActiveState=active\nUnitFileState=enabled'),self.assertRaisesRegex(RuntimeError,'not fenced'):r.legacy_health_gate(m)

    def test_shared_lock_timeout_never_removes_foreign_marker(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);(root/'base').mkdir();(root/'receipt').mkdir();marker=root/'base/.review-write.lock';marker.write_text('foreign')
            with self.assertRaisesRegex(RuntimeError,'existing shared write lock'):
                r.acquire_deploy_fence({'base':str(root/'base'),'release_id':'fixture'},root/'receipt',0)
            self.assertEqual(marker.read_text(),'foreign')
            self.assertFalse((root/'receipt/fence-owned.json').exists())

    def test_fence_recovery_releases_only_exact_recorded_inode_and_bytes(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);(root/'base').mkdir();(root/'receipt').mkdir();m={'base':str(root/'base'),'release_id':'fixture'}
            r.acquire_deploy_fence(m,root/'receipt',0);marker=root/'base/.review-write.lock'
            marker.write_text('foreign replacement contents')
            with patch.object(r,'check_peer_drain'),self.assertRaisesRegex(RuntimeError,'changed'):r.release_deploy_fence(m,root/'receipt')
            self.assertEqual(marker.read_text(),'foreign replacement contents')

    def test_unresolved_legacy_journal_cannot_be_acknowledged_as_terminal(self):
        import hashlib
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            base=Path(tmp);folder=base/'person-operations/fixture';folder.mkdir(parents=True);journal=folder/'operation.json';journal.write_text('{"status":"moving"}')
            m={'base':str(base),'legacy_terminal_acknowledgements':{'person-operations/fixture/operation.json':hashlib.sha256(journal.read_bytes()).hexdigest()}}
            with self.assertRaisesRegex(RuntimeError,'unresolved'):r.legacy_journals(m)
            journal.write_text('{"status":"rolled_back"}');self.assertEqual(len(r.legacy_journals(m)),1)

    def test_fence_recovery_refuses_replaced_inode_even_with_identical_bytes(self):
        with tempfile.TemporaryDirectory(prefix='face-release-fixture-') as tmp:
            root=Path(tmp);(root/'base').mkdir();(root/'receipt').mkdir();m={'base':str(root/'base'),'release_id':'fixture'}
            r.acquire_deploy_fence(m,root/'receipt',0);marker=root/'base/.review-write.lock';raw=marker.read_bytes()
            marker.rename(root/'original-owned-lock');marker.write_bytes(raw)
            with patch.object(r,'check_peer_drain'),self.assertRaisesRegex(RuntimeError,'changed'):r.release_deploy_fence(m,root/'receipt')
            self.assertEqual(marker.read_bytes(),raw)

if __name__=='__main__':unittest.main()
