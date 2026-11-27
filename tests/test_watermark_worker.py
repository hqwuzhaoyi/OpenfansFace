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
                                     ('X=@ne__6i', 'X', 'ne__6i'),
                                     ('推特 @alice_x', 'X', 'alice_x'),
                                     ('https://myfans.jp/secret_japan', 'MyFans', 'secret_japan'),
                                     ('candfans.jp/creator_1', 'CandFans', 'creator_1'),
                                     ('作者 @creator_99', 'unknown', '@creator_99'),
                                     ('ig: @yuumei_daily', 'Instagram', 'yuumei_daily'),
                                     ('patreon.com/laurus0711', 'Patreon', 'laurus0711')]:
            identity = text_identity(text)
            self.assertEqual((identity['platform'], identity['token'], identity['kind']), (platform, token, 'account'))
        self.assertEqual(text_identity('ooyq@aas.com/papaxmama')['kind'], 'text')

    def test_case_insensitive_vote_merge_and_faint_three_frame_threshold(self):
        def observed(text, frame, score):
            return dict(text=text, frame=frame, score=score, timestamp=frame, evidence='example.jpg')
        # Case variants merge votes while keeping highest-scoring casing
        merged = aggregate([observed('Comerzz', 0, .82), observed('ComerZz', 1, .94)])
        self.assertEqual(len(merged), 1)
        self.assertEqual((merged[0]['token'], merged[0]['frames']), ('ComerZz', 2))
        # Faint 0.50-0.59 detections require 3 frames; 2 faint frames or <0.50 stay rejected
        self.assertEqual(aggregate([observed('@faint', 0, .54), observed('@faint', 1, .56)]), [])
        self.assertEqual(aggregate([observed('@too_low', 0, .45), observed('@too_low', 1, .45), observed('@too_low', 2, .45)]), [])
        promoted = aggregate([observed('@faint', 0, .54), observed('@faint', 1, .56), observed('@faint', 2, .55)])
        self.assertEqual(len(promoted), 1)
        self.assertEqual(promoted[0]['frames'], 3)

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

    @unittest.skipUnless(find_spec('PIL'), 'Pillow needed for contrast fallback test')
    def test_scan_video_contrast_fallback_recovers_faint_watermarks(self):
        from PIL import Image, ImageStat
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            video = root / 'faint.mp4'
            video.write_bytes(b'video')
            def run(command, **kwargs):
                if command[0] == 'ffprobe':
                    return SimpleNamespace(stdout='{"format":{"duration":10}}')
                img = Image.new('RGB', (160, 120), (120, 120, 120))
                img.putpixel((10, 10), (200, 200, 200))
                img.save(command[-1])
                return SimpleNamespace(returncode=0)
            def recognize(image):
                # Non-enhanced image has low contrast (min 120 -> max 200); autocontrast stretches pixels to 0..255
                extrema = ImageStat.Stat(image).extrema
                if extrema[0][0] == 0:
                    return [dict(text='myfans.jp/faint_star', score=0.72, box=[5, 5, 60, 25])]
                return []
            engine = SimpleNamespace(recognize=recognize)
            with patch('maintenance.watermark_worker.subprocess.run', side_effect=run):
                res = scan_video(dict(fingerprint(video), vid='faint'), root, root, engine, 'job-faint')
            self.assertEqual(res['sampled_frames'], 8)
            self.assertEqual(len(res['candidates']), 1)
            self.assertEqual((res['candidates'][0]['platform'], res['candidates'][0]['token']), ('MyFans', 'faint_star'))


if __name__ == '__main__':
    unittest.main()
