import fs from 'node:fs'
import path from 'node:path'
import { BASE } from './env'
import { canonicalPersonName, personRenames } from './person-names'
import { parsePersonAccounts, type PersonAccountLink } from './person-account-model'

export const PERSON_ACCOUNTS_FILE=path.join(BASE,'external-profiles/manual-links.json')
export function loadPersonAccounts():PersonAccountLink[] {
  let raw:string
  try{raw=fs.readFileSync(PERSON_ACCOUNTS_FILE,'utf8')}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return [];throw error}
  const renames=personRenames()
  return parsePersonAccounts(JSON.parse(raw)).map(link=>({...link,creator:canonicalPersonName(link.creator,renames)}))
}
