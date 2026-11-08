"""Regression for raw OCR candidates and non-pikpak indexed videos; isolated fixtures only."""
import importlib.util
import json
from pathlib import Path
import sqlite3


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


archive = module('archive_fixture_regression', 'http-rust-archive.py')
publication = module('publication_fixture_regression', 'http-rust-processing-result.py')


def raw_candidate_is_evidence_not_a_creator_path():
    f = archive.Fixture(2)
    try:
        with sqlite3.connect(f.base / 'processing/processing.db') as db:
            data = json.loads(db.execute('SELECT data FROM tasks WHERE vid=?', (f.ids[0],)).fetchone()[0])
            data['candidates'] = [{'name':'The.','source':'watermark','confirmed':False,'detail':'OCR raw'}]
            db.execute('UPDATE tasks SET data=? WHERE vid=?', (json.dumps(data), f.ids[0]))
        page = f.call('review-page')
        row = next(r for r in page['rows'] if r['vid'] == f.ids[0])
        assert row['data']['candidates'][0]['name'] == 'The.'
        assert '__review_raw_people' not in row
        body = {'creator':'Momoyih','vids':[row['vid']],'source':'review','review_session':page['session'],
                'review_tokens':{row['vid']:row['review_token']},
                'review_group':{'key':page['group']['key'],'deferred':bool(row['deferred'])},
                'fingerprints':{row['vid']:{k:row[k] for k in ('path','size','mtime_ns')}}}
        invalid = {**body, 'creator':'The.'}
        f.call('queue-confirm', invalid, status=400)
        assert f.call('queue-confirm', body)['added'] == 1
        print('PASS raw OCR preserved; invalid creator still rejected; explicit confirmation works')
    finally:
        f.stop()


def an_invalid_candidate_alias_change_invalidates_old_evidence():
    cases = [({}, {'The.':'AnotherPerson'}),
             ({'The.':'Mid.'}, {'The.':'Else.'}),
             ({'The.':'Momoyih'}, {'The.':'AnotherPerson'})]
    for before, after in cases:
        f = archive.Fixture(1)
        try:
            archive.write(f.base / 'person-renames.json', before)
            with sqlite3.connect(f.base / 'processing/processing.db') as db:
                data = json.loads(db.execute('SELECT data FROM tasks WHERE vid=?', (f.ids[0],)).fetchone()[0])
                data['candidates'] = [{'name':'The.','source':'watermark','confirmed':False}]
                db.execute('UPDATE tasks SET data=? WHERE vid=?', (json.dumps(data), f.ids[0]))
            page = f.call('review-page')
            row = page['rows'][0]
            body = {'creator':'Momoyih','vids':[row['vid']],'source':'review','review_session':page['session'],
                    'review_tokens':{row['vid']:row['review_token']},
                    'review_group':{'key':page['group']['key'],'deferred':bool(row['deferred'])},
                    'fingerprints':{row['vid']:{k:row[k] for k in ('path','size','mtime_ns')}}}
            archive.write(f.base / 'person-renames.json', after)
            f.call('queue-confirm', body, status=409)
            assert not f.queue()
        finally:
            f.stop()
    print('PASS raw candidate alias changes invalidate previously delivered evidence, including chained and valid aliases')


def unrelated_alias_changes_do_not_block_confirmation():
    f = archive.Fixture(1)
    try:
        with sqlite3.connect(f.base / 'processing/processing.db') as db:
            data = json.loads(db.execute('SELECT data FROM tasks WHERE vid=?', (f.ids[0],)).fetchone()[0])
            data['candidates'] = [{'name':'The.','source':'watermark','confirmed':False}]
            db.execute('UPDATE tasks SET data=? WHERE vid=?', (json.dumps(data), f.ids[0]))
        page = f.call('review-page');row = page['rows'][0]
        body = {'creator':'Momoyih','vids':[row['vid']],'source':'review','review_session':page['session'],
                'review_tokens':{row['vid']:row['review_token']},
                'review_group':{'key':page['group']['key'],'deferred':bool(row['deferred'])},
                'fingerprints':{row['vid']:{k:row[k] for k in ('path','size','mtime_ns')}}}
        archive.write(f.base / 'person-renames.json', {'Unrelated.':'Other'})
        assert f.call('queue-confirm', body)['added'] == 1
        print('PASS unrelated alias updates preserve valid review evidence')
    finally:
        f.stop()


def non_target_library_does_not_break_reference_state():
    x = publication.Fixture()
    try:
        other = x.media / 'jav' / 'unrelated.mp4'
        other.parent.mkdir(parents=True)
        other.write_bytes(b'other library original')
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)', ('other-library', str(other), other.name))
        initial = x.meta('analysis-state')['version']
        other.write_bytes(b'other library changed')
        assert x.meta('analysis-state')['version'] == initial
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('UPDATE videos SET path=? WHERE vid=?', (str(other.with_name('changed.mp4')), 'other-library'))
        assert x.meta('analysis-state')['version'] != initial
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('ALTER TABLE faces ADD COLUMN emb BLOB')
            db.execute('INSERT INTO faces(vid,score,thumb,emb) VALUES(?,?,?,?)', ('other-library',.9,'outside-thumb',b'\0'*2048))
            db.execute('UPDATE videos SET path=? WHERE vid=?', (str(other), 'other-library'))
        assert x.meta('analysis-state')['version']
        publication.f.write(x.base / 'integration-queue.json',[{'vid':'other-library','creator':'Unrelated'}])
        x.meta('analysis-state', status=409)
        publication.f.write(x.base / 'integration-queue.json',[])
        prior_report=json.loads((x.base/'report.json').read_text())
        publication.f.write(x.base / 'report.json',{'identities':[{'name':'Anchor','rep_thumb':'outside-thumb'}]})
        x.meta('analysis-state', status=409)
        publication.f.write(x.base / 'report.json',prior_report)
        directory = x.media / 'pikpak/My Telegram/not-a-video.mp4'
        directory.mkdir()
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)', ('directory', str(directory), directory.name))
        directory_version = x.meta('analysis-state')['version']
        (directory / 'unrelated.txt').write_text('does not affect video references')
        assert x.meta('analysis-state')['version'] == directory_version
        x.meta('state', {'scope':{'vids':['directory'],'people':[],'watermark_keys':[]}}, status=409)
        creator_directory = x.media / 'pikpak/creators/甲/not-a-video.mp4'
        creator_directory.mkdir()
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)', ('creator-directory', str(creator_directory), creator_directory.name))
            db.execute('INSERT INTO faces(vid,score,thumb,emb) VALUES(?,?,?,?)', ('creator-directory',.9,'creator-thumb',b'\0'*2048))
        x.meta('analysis-state', status=409)
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('DELETE FROM faces WHERE vid=?', ('creator-directory',))
        assert x.meta('analysis-state')['version']
        link = x.media / 'pikpak/My Telegram/linked.mp4'
        link.symlink_to(other)
        with sqlite3.connect(x.base / 'faces.db') as db:
            db.execute('INSERT INTO videos(vid,path,name) VALUES(?,?,?)', ('unsafe-link', str(link), link.name))
        x.meta('analysis-state', status=409)
        print('PASS unrelated library and empty non-regular entries do not block references; assigned/anchor inputs and selected files remain guarded')
    finally:
        x.stop()


if __name__ == '__main__':
    raw_candidate_is_evidence_not_a_creator_path()
    an_invalid_candidate_alias_change_invalidates_old_evidence()
    unrelated_alias_changes_do_not_block_confirmation()
    non_target_library_does_not_break_reference_state()
