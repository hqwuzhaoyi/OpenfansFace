import json
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'maintenance'))
from task_progress import TaskProgress


class ProgressTests(unittest.TestCase):
    def test_parent_completion_preserves_child_frame_progress(self):
        with tempfile.TemporaryDirectory() as directory:
            parent=TaskProgress(directory,'text','提取水印',job_id='one')
            child=TaskProgress(directory,'text','提取水印',job_id='one',attach=True)
            child.update(done=8,total=8,unit='帧',stage='汇总')
            parent.finish()
            saved=json.loads((Path(directory)/'jobs/text.json').read_text())
            self.assertEqual(saved['done'],8)
            self.assertEqual(saved['status'],'complete')
            self.assertTrue(saved['finished_at']>=saved['started_at'])
            self.assertEqual(list((Path(directory)/'jobs').glob('.text.json*')),[])

    def test_new_job_does_not_reuse_previous_progress(self):
        with tempfile.TemporaryDirectory() as directory:
            first=TaskProgress(directory,'face','人脸分析',job_id='old')
            first.update(done=7,total=8)
            second=TaskProgress(directory,'face','人脸分析',job_id='new',attach=True)
            second.finish(ValueError('读取失败'))
            saved=json.loads((Path(directory)/'jobs/face.json').read_text())
            self.assertEqual(saved['id'],'new')
            self.assertIsNone(saved['done'])
            self.assertEqual(saved['status'],'error')
            self.assertEqual(saved['error'],'读取失败')
