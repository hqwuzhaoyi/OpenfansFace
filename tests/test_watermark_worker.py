import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from importlib.util import find_spec
from types import SimpleNamespace

from maintenance.watermark_worker import PIPELINE, aggregate, fingerprint, process_job, sample_times, scan_video, text_identity, validate_source


class WatermarkWorkerTests(unittest.TestCase):
    def test_distinct_frames_not_crop_count(self):
        def observed(text, frame, score=.95):
            return dict(text=text, frame=frame, score=score, timestamp=frame, evidence='example.jpg')
        self.assertEqual(aggregate([observed('@alice', 0)] * 5), [])
        rows = aggregate([observed('@alice', 0), observed('@alice', 1), observed('@alice', 1), observed('@noise', 2)])
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]['frames'], 2)
        self.assertEqual(rows[0]['token'], '@alice')
        self.assertEqual(aggregate([observed('@alice', 0, .4), observed('@alice', 1, .4)]), [])

    def test_platform_and_punctuation_separate_accounts(self):
        self.assertNotEqual(text_identity('@a.b')['key'], text_identity('@a_b')['key'])
        self.assertNotEqual(text_identity('抖音号: alice')['key'], text_identity('快手号: alice')['key'])
        self.assertEqual(text_identity('抖音号：alice')['key'], text_identity('抖音号:alice')['key'])
        self.assertEqual(text_identity('转载频道')['kind'], 'text')
        self.assertIsNone(text_identity('00:12'))

    def test_creator_urls_and_repost_handles_are_accounts_not_verified_people(self):
        for text, platform, token in [('OnlyFans.com/papaxmama', 'OnlyFans', 'papaxmama'),
                                     ('t.me/MroHome', 'Telegram', 'MroHome'),
                                     ('T.ME/COSDHZ', 'Telegram', 'COSDHZ'),
                                     ('TG@ysjcc', 'Telegram', 'ysjcc'),
                                     ('onlyfans@cocopie0218', 'OnlyFans', 'cocopie0218'),
                                     ('X:@yuumeilyn', 'X', 'yuumeilyn'),
                                     ('ig: @yuumei_daily', 'Instagram', 'yuumei_daily'),
                                     ('patreon.com/laurus0711', 'Patreon', 'laurus0711')]:
            identity = text_identity(text)
            self.assertEqual((identity['platform'], identity['token'], identity['kind']), (platform, token, 'account'))
        self.assertEqual(text_identity('ooyq@aas.com/papaxmama')['kind'], 'text')

    def test_sampling_covers_video_and_rejects_invalid_duration(self):
        samples = sample_times(100)
        self.assertEqual(len(samples), 8)
        self.assertLess(samples[0], 5)
        self.assertGreater(samples[-1], 90)
        self.assertTrue(all(0 <= t < .03 for t in sample_times(.03)))
        for duration in (0, -1, float('inf'), float('nan')):
            with self.assertRaises(ValueError):
                sample_times(duration)

    def test_file_containment_and_fingerprint(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            media = root / 'media'
            media.mkdir()
            video = media / 'one.mp4'
            video.write_bytes(b'video')
            item = fingerprint(video)
            self.assertEqual(validate_source(item, media), video)
            video.write_bytes(b'replacement')
            with self.assertRaises(ValueError):
                validate_source(item, media)
            outside = root / 'outside.mp4'
            outside.write_bytes(b'other')
            link = media / 'link.mp4'
            link.symlink_to(outside)
            with self.assertRaises(ValueError):
                validate_source(fingerprint(link), media)
            organized = media / 'pikpak/creators/A/video.mp4'
            organized.parent.mkdir(parents=True)
            organized.write_bytes(b'video')
            with self.assertRaises(ValueError):
                validate_source(fingerprint(organized), media)

    def test_legacy_job_cannot_publish_even_without_native_environment(self):
        from maintenance.watermark_worker import publish
        with tempfile.TemporaryDirectory(prefix='face-watermark-fixture-') as temp:
            root=Path(temp);directory=root/'watermarks';directory.mkdir()
            with patch.dict('os.environ', {}, clear=True):
                with self.assertRaisesRegex(RuntimeError, '已退役'):
                    process_job({'id':'retired'},directory,root)
                with self.assertRaisesRegex(RuntimeError, '原生协调器'):
                    publish(directory/'results.json',{})
            self.assertEqual(list(directory.iterdir()), [])

    @unittest.skipUnless(find_spec('PIL'), 'Pillow needed for frame extraction regression')
    def test_empty_ffmpeg_output_cannot_reuse_previous_frame(self):
        from PIL import Image
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            video = root / 'one.mp4'
            video.write_bytes(b'video')
            calls = []
            def run(command, **kwargs):
                if command[0] == 'ffprobe':
                    return SimpleNamespace(stdout='{"format":{"duration":3}}')
                calls.append(command)
                if len(calls) == 1:
                    Image.new('RGB', (120, 90)).save(command[-1])
                return SimpleNamespace(returncode=0)
            engine = SimpleNamespace(recognize=lambda image: [])
            with patch('maintenance.watermark_worker.subprocess.run', side_effect=run):
                with self.assertRaisesRegex(ValueError, '有效视频帧不足'):
                    scan_video(dict(fingerprint(video), vid='one'), root, root, engine, 'job')


if __name__ == '__main__':
    unittest.main()
