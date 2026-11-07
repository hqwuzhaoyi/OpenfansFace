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
            with patch.object(r,'validate'),patch.object(r,'capacity'),patch.object(r,'baseline',return_value=original),patch.object(r,'gpu_probe'),patch.object(r,'check_peer_drain'),patch.object(r,'drain',return_value={'drained':True}),patch.object(r,'authority'),patch.object(r,'snapshot_tree'),patch.object(r,'backup_database'),patch.object(r,'inspect',return_value=original['native']),patch.object(r,'run',side_effect=command),self.assertRaisesRegex(RuntimeError,'creation failed'):
                r.replace(m,receipt)
            self.assertTrue((receipt/'native-stopped.json').exists())
            self.assertFalse((receipt/'native-started.json').exists())
            self.assertFalse((receipt/'worker-stopped.json').exists())
            self.assertFalse(any(args[:2]==('docker','start') for args in calls))

if __name__=='__main__':unittest.main()
