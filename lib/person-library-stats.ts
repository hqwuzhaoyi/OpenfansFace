import { rustRequest } from './file-operation-backend'
import type { PersonDirectoryStats } from './person-library-model'

export const personVideoExtension=/\.(mp4|m4v|mov|mkv|webm|avi|wmv|ts|mts|m2ts)$/i
type Result={stats:Record<string,PersonDirectoryStats>;updated_at:string}
// Native reads are fresh and bounded; no Node directory walk or stale filesystem cache.
export function invalidatePersonStats(){}
export async function personDirectoryStats(force=false):Promise<Result>{
  return rustRequest<Result>(null,undefined,'v1/person-library?stats=1'+(force?'&refresh=1':''))
}
