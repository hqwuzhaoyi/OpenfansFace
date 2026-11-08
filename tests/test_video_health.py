import datetime as dt, importlib.util, json, sqlite3, tempfile, unittest
from pathlib import Path
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('health',Path(__file__).parents[1]/'maintenance/video_health.py')
health=importlib.util.module_from_spec(spec);spec.loader.exec_module(health)
NOW=dt.datetime(2026,11,5,19,30,tzinfo=dt.timezone.utc)

class PolicyTests(unittest.TestCase):
    def test_requires_three_separated_checks_and_recovery_resets_visibility(self):
        observation=dict(status='corrupt',path='/media/test.mp4',size=10,mtime_ns='1000')
        first=health.transition({},observation,NOW)
        self.assertFalse(first['hidden'])
        repeated=health.transition(first,observation,NOW+dt.timedelta(minutes=10))
        self.assertEqual(repeated['consecutive'],1)
        second=health.transition(repeated,observation,NOW+dt.timedelta(days=1))
        self.assertFalse(second['hidden'])
        third=health.transition(second,observation,NOW+dt.timedelta(days=2))
        self.assertTrue(third['hidden'])
        changed=health.transition(third,{**observation,'mtime_ns':'2000'},NOW+dt.timedelta(days=3))
        self.assertFalse(changed['hidden']);self.assertEqual(changed['consecutive'],1)
        for state in ['healthy','compatibility','playback_warning','unavailable']:
            self.assertFalse(health.transition(third,{**observation,'status':state},NOW)['hidden'])

    def test_video_probe_distinguishes_structural_failure_from_io_and_codec(self):
        def result(code,data,error=''):
            return type('Result',(),dict(returncode=code,stdout=json.dumps(data),stderr=error))()
        cases=[(result(1,{},'moov atom not found'),'corrupt'),
               (result(1,{},'Input/output error'),'unavailable'),
               (result(0,dict(streams=[])),'corrupt'),
               (result(0,dict(streams=[dict(codec_type='video',codec_name='h264',pix_fmt='yuv420p')])),'healthy'),
               (result(0,dict(streams=[dict(codec_type='video',codec_name='wmv3',pix_fmt='yuv420p')])),'compatibility')]
        for mocked,expected in cases:
            with patch.object(health.subprocess,'run',return_value=mocked): self.assertEqual(health.probe('/unused')['status'],expected)

class ScanTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory(prefix='face-health-',dir='/tmp')
        self.home=Path(self.temp.name);(self.home/'.fixture-marker').write_text('fixture')
        self.base=self.home/'base';self.base.mkdir();self.root=self.home/'media'
        for folder in ['creators/known','My Telegram/test']:
            (self.root/'pikpak'/folder).mkdir(parents=True)
        (self.root/'pikpak/creators/known/good.mp4').write_bytes(b'known organized file')
        self.file=self.root/'pikpak/My Telegram/test/good.mp4';self.file.write_bytes(b'video-content')
        self.good=r'R:\media\pikpak\My Telegram\test\good.mp4'
        self.missing=r'R:\media\pikpak\My Telegram\test\missing.mp4'
        con=sqlite3.connect(self.base/'faces.db');con.execute('CREATE TABLE videos(vid TEXT,path TEXT)')
        con.executemany('INSERT INTO videos VALUES (?,?)',[(self.good,str(self.file)),(self.missing,str(self.file.parent/'missing.mp4'))]);con.commit();con.close()
        (self.base/'report.json').write_text(json.dumps(dict(identities=[dict(vids=[dict(vid=self.good),dict(vid=self.missing)])])))
        self.original_db=(self.base/'faces.db').read_bytes();self.original_media=self.file.read_bytes()
    def tearDown(self):self.temp.cleanup()
    def test_retired_publisher_cannot_write_even_without_native_environment(self):
        with patch.dict('os.environ',{},clear=True):
            with self.assertRaisesRegex(RuntimeError,'legacy publication is disabled'):
                health.run(self.base,self.root,'fixture',fixture=True)
            with self.assertRaisesRegex(RuntimeError,'legacy publication is disabled'):
                health.publish(self.base/'video-health.json',{})
        self.assertFalse((self.base/'video-health.json').exists())
        self.assertEqual((self.base/'faces.db').read_bytes(),self.original_db)
        self.assertEqual(self.file.read_bytes(),self.original_media)

if __name__=='__main__':unittest.main()
