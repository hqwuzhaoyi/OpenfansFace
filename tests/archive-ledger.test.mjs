import {test} from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {execFileSync} from 'node:child_process'
import {archiveLedgerProgram} from '../lib/archive-ledger-program.ts'

function fixture(t){
 const dir=fs.mkdtempSync(path.join(os.tmpdir(),'face-archive-ledger-test-')),database=path.join(dir,'ledger.db')
 t.after(()=>fs.rmSync(dir,{recursive:true,force:true}))
 const sql=`CREATE TABLE asset(id INTEGER PRIMARY KEY,path TEXT,disposal TEXT);
 CREATE TABLE entity(id INTEGER PRIMARY KEY,kind TEXT,canonical_name TEXT,normalized_name TEXT,metadata_json TEXT,created_at TEXT,updated_at TEXT,UNIQUE(kind,normalized_name));
 CREATE TABLE asset_entity(asset_id INTEGER,entity_id INTEGER,role TEXT,source TEXT,confidence REAL,metadata_json TEXT,first_seen_at TEXT,last_seen_at TEXT);
 INSERT INTO entity VALUES(7,'creator','momoyih','momoyih','{}','','');
 INSERT INTO asset VALUES(1,'source',NULL);`
 execFileSync('python3',['-c','import sqlite3,sys;c=sqlite3.connect(sys.argv[1]);c.executescript(sys.argv[2]);c.commit()',database,sql])
 const rows=[{from:'source',to:'destination',creator:'Momoyih'}]
 const run=(action,id='fixture',values=rows)=>JSON.parse(execFileSync('python3',['-c',archiveLedgerProgram,Buffer.from(JSON.stringify({database,action,id,rows:values})).toString('base64')],{stdio:['ignore','pipe','pipe']}).toString())
 const query=sql=>JSON.parse(execFileSync('python3',['-c','import sqlite3,sys,json;c=sqlite3.connect(sys.argv[1]);print(json.dumps(c.execute(sys.argv[2]).fetchall()))',database,sql]).toString())
 return {run,query,rows}
}
test('archive reuses normalized creator identity and committed receipts are idempotent',t=>{
 const {run,query}=fixture(t)
 assert.equal(run('reserve').state,'reserved')
 assert.deepEqual(run('apply'),{state:'committed',updated:1})
 assert.deepEqual(run('status'),{state:'committed',updated:1})
 assert.deepEqual(run('apply'),{state:'committed',updated:1})
 assert.deepEqual(query('SELECT id,canonical_name FROM entity'),[[7,'momoyih']])
 assert.deepEqual(query('SELECT asset_id,entity_id FROM asset_entity'),[[1,7]])
 assert.deepEqual(query('SELECT path FROM asset'),[['destination']])
})
test('missing assets fail before any move; status cancels a late request',t=>{
 const {run,query,rows}=fixture(t)
 assert.throws(()=>run('reserve','missing',[{...rows[0],from:'missing'}]))
 assert.deepEqual(query('SELECT path FROM asset'),[['source']])
 run('reserve');assert.equal(run('status').state,'cancelled')
 assert.throws(()=>run('apply'));assert.deepEqual(query('SELECT path FROM asset'),[['source']])
 assert.equal(run('status','never-started').state,'cancelled')
 assert.throws(()=>run('reserve','never-started'))
})
