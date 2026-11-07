/** Executed in Peach (or an isolated local ledger in tests). No media files are touched. */
export const archiveLedgerProgram = `import json,sqlite3,base64,datetime,sys,os
p=json.loads(base64.b64decode(''.join(sys.argv[1:])))
if not os.path.isfile(p['database']):raise ValueError('Peach 账本不存在')
con=sqlite3.connect(p['database'],timeout=30)
con.row_factory=sqlite3.Row
now=datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
def creator(name):
 rows=con.execute("SELECT id FROM entity WHERE kind='creator' AND (canonical_name=? OR normalized_name=?)",(name,name.lower())).fetchall()
 if len(rows)>1:raise ValueError('人物名称存在歧义：'+name)
 return rows[0]['id'] if rows else None
def validate(rows):
 for m in rows:
  if len(con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['from'],)).fetchall())!=1:raise ValueError('源视频账本记录缺失或不唯一：'+m['from'])
  if con.execute("SELECT 1 FROM asset WHERE path=? AND disposal IS NULL",(m['to'],)).fetchone():raise ValueError('目标路径已有账本记录：'+m['to'])
  if m.get('canonical') and len(con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['canonical'],)).fetchall())!=1:raise ValueError('重复件目标账本记录缺失或不唯一')
  creator(m['creator'])
try:
 con.execute('BEGIN IMMEDIATE')
 con.execute('CREATE TABLE IF NOT EXISTS _face_admin_archive_ops(id TEXT PRIMARY KEY,state TEXT NOT NULL,rows_json TEXT NOT NULL,updated INTEGER NOT NULL DEFAULT 0)')
 op=con.execute('SELECT * FROM _face_admin_archive_ops WHERE id=?',(p['id'],)).fetchone()
 if p['action']=='reserve':
  if op:raise ValueError('归档编号已使用，请核对原操作结果')
  validate(p['rows'])
  con.execute("INSERT INTO _face_admin_archive_ops(id,state,rows_json) VALUES(?,'reserved',?)",(p['id'],json.dumps(p['rows'])))
  result=dict(state='reserved')
 elif p['action']=='status':
  # Serialize with a late apply. Once cancelled, the same operation cannot commit.
  if not op:con.execute("INSERT INTO _face_admin_archive_ops(id,state,rows_json) VALUES(?,'cancelled','[]')",(p['id'],))
  elif op['state']=='reserved':con.execute("UPDATE _face_admin_archive_ops SET state='cancelled' WHERE id=?",(p['id'],))
  result=dict(state='committed' if op and op['state']=='committed' else 'cancelled',updated=op['updated'] if op else 0)
 elif p['action']=='apply':
  if not op or op['state']=='cancelled':raise ValueError('归档已取消，拒绝迟到提交')
  if op['state']=='committed':result=dict(state='committed',updated=op['updated'])
  else:
   reserved=json.loads(op['rows_json'])
   if any(m not in reserved for m in p['rows']):raise ValueError('归档范围与预留记录不一致')
   validate(p['rows'])
   for m in p['rows']:
    eid=creator(m['creator'])
    if eid is None:eid=con.execute("INSERT INTO entity(kind,canonical_name,normalized_name,metadata_json,created_at,updated_at) VALUES('creator',?,?,?,?,?)",(m['creator'],m['creator'].lower(),json.dumps({'source':'face-curation-8919'}),now,now)).lastrowid
    aid=con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['from'],)).fetchone()['id']
    con.execute('UPDATE asset SET path=? WHERE id=?',(m['to'],aid))
    linked=[aid]
    if m.get('canonical'):linked.append(con.execute("SELECT id FROM asset WHERE path=? AND disposal IS NULL",(m['canonical'],)).fetchone()['id'])
    for asset in linked:
     if not con.execute("SELECT 1 FROM asset_entity WHERE asset_id=? AND entity_id=? AND role='creator'",(asset,eid)).fetchone():
      con.execute("INSERT INTO asset_entity(asset_id,entity_id,role,source,confidence,metadata_json,first_seen_at,last_seen_at) VALUES(?,?,?,?,?,?,?,?)",(asset,eid,'creator','face-curation-8919',0.95,'{}',now,now))
   con.execute("UPDATE _face_admin_archive_ops SET state='committed',updated=? WHERE id=?",(len(p['rows']),p['id']))
   result=dict(state='committed',updated=len(p['rows']))
 else:raise ValueError('未知账本操作')
 con.commit();print(json.dumps(result))
except Exception:
 con.rollback();raise
finally:con.close()
`
