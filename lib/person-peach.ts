import Database from 'better-sqlite3'
import { peachExec } from './peach'
import { canonicalPersonName, personRenames } from './person-names'
import { validateCreator } from './review-logic'
import { peachPublicBase, validatePeachPersonId, type PeachPersonLookup } from './person-peach-model'

type Row={id:string;name:string}
/** Resolve an existing creator record only; public profile guesses never establish a Peach link. */
export async function lookupPeachPerson(input:string):Promise<PeachPersonLookup>{
  const requested=validateCreator(input),names=personRenames()
  if(!names||typeof names!=='object'||Array.isArray(names)||Object.values(names).some(value=>typeof value!=='string'))throw new Error('人物旧名记录格式异常')
  const name=validateCreator(canonicalPersonName(requested,names))
  const aliases=[...new Set([name,...Object.keys(names).filter(alias=>canonicalPersonName(alias,names)===name)])]
  if(aliases.length>400)throw new Error('人物旧名数量超出查询范围')
  const marks=aliases.map(()=>'?').join(',')
  const queries=[
    {sql:"SELECT CAST(id AS TEXT) AS id,canonical_name AS name FROM entity WHERE kind='creator' AND canonical_name=? ORDER BY id LIMIT 2",args:[name]},
    {sql:`SELECT CAST(e.id AS TEXT) AS id,e.canonical_name AS name FROM entity e WHERE e.kind='creator' AND (e.canonical_name IN (${marks}) OR EXISTS(SELECT 1 FROM entity_alias a WHERE a.entity_id=e.id AND a.alias IN (${marks}))) ORDER BY e.id LIMIT 2`,args:[...aliases,...aliases]},
  ]
  const base_url=peachPublicBase(process.env.PEACH_PUBLIC_URL)
  let rows:Row[]=[]
  if(process.env.PEACH_LEDGER_PATH){
    const db=new Database(process.env.PEACH_LEDGER_PATH,{readonly:true,fileMustExist:true,timeout:1000})
    try{
      db.pragma('query_only=ON')
      rows=db.transaction(()=>{
        for(const query of queries){const found=db.prepare(query.sql).all(...query.args) as Row[];if(found.length)return found}
        return []
      })()
    }finally{db.close()}
  }else{
    const program=`import json,sqlite3,sys,signal\nsignal.alarm(6)\nc=sqlite3.connect('file:/data/database/ledger.db?mode=ro',uri=True,timeout=2);c.row_factory=sqlite3.Row\nc.execute('PRAGMA query_only=ON');c.execute('BEGIN')\ntry:\n rows=[]\n for query in json.loads(sys.argv[1]):\n  rows=[dict(row) for row in c.execute(query['sql'],query['args'])]\n  if rows:break\n print(json.dumps(rows,ensure_ascii=False))\nfinally:c.close()`
    let timer:ReturnType<typeof setTimeout>|undefined
    try{
      const response=await Promise.race([peachExec(program,8000,[JSON.stringify(queries)]),new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(new Error('Peach 查询超时')),8000)})])
      if(response.code!==0)throw new Error('Peach 人物记录暂时无法读取')
      rows=JSON.parse(response.out)
    }finally{clearTimeout(timer)}
  }
  if(!Array.isArray(rows)||rows.some(row=>typeof row.id!=='string'||!/^\d+$/.test(row.id)||typeof row.name!=='string'))throw new Error('Peach 人物记录格式异常')
  for(const row of rows)validatePeachPersonId(row.id)
  if(rows.length!==1)return {status:rows.length?'ambiguous':'missing',base_url}
  return {status:'linked',person:{...rows[0],kind:'creator'},base_url}
}
