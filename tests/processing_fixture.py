"""Test-only old result seeding. Production worker publishes solely through Rust."""
import json
from pathlib import Path
import tempfile
import time
from processing_logic import evaluate_stage
from processing_store import GUARD, guard, evidence_flags, event, close_run

def finish(con, task, data, next_stage=None):
    database = Path(con.execute('PRAGMA database_list').fetchone()[2]).resolve()
    root = database.parent.parent
    if not root.is_relative_to(Path(tempfile.gettempdir()).resolve()) or (root / '.processing-fixture').read_text() != 'isolated processing fixture':
        raise RuntimeError('fixture publisher requires a marked temporary database')
    if next_stage is None:
        evaluated = evaluate_stage(root, task, data)
        data, next_stage = evaluated['data'], evaluated['next_stage']
    result = con.execute(f'UPDATE tasks SET data=?,stage=?,error=NULL,next_retry=0,updated_at=? WHERE {GUARD}',
                         (json.dumps(data, ensure_ascii=False), next_stage, time.time(), *guard(task)))
    if result.rowcount:
        flags = evidence_flags(data)
        con.execute('''UPDATE batch_items SET text_done=?,face_required=?,face_done=?,face_skipped=?,completed=?,
            completed_at=CASE WHEN ?=1 THEN ? ELSE NULL END WHERE batch_id=? AND vid=? AND completed=0''',
                    (*flags, flags[-1], time.time(), task['batch_id'], task['vid']))
        event(con, task['vid'], 'ready' if next_stage == 'review' else 'stage_complete',
              '分析完成，等待人工确认' if next_stage == 'review' else '文字线索完成，等待人脸分析')
    close_run(con, task, 'done' if result.rowcount else 'superseded')
    con.commit()


