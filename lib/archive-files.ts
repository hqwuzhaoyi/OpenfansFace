import fs from 'node:fs'
import path from 'node:path'
import { createHash } from 'node:crypto'

export function validateArchiveName(name: unknown): string {
  if (typeof name !== 'string' || name !== name.trim() || !name || Buffer.byteLength(name) > 240 || /[\\/:*?"<>|\x00-\x1f]/.test(name) || name.endsWith('.') || ['.', '..'].includes(name)) {
    throw new Error('文件名无效，请勿包含路径分隔符或特殊字符')
  }
  return name
}

export function alternateName(name: string, vid: string): string {
  const ext = path.extname(name), stem = path.basename(name, ext)
  const suffix = ` (保留两份-${createHash('sha256').update(vid).digest('hex').slice(0, 8)})${ext}`
  let shortened = stem
  while (Buffer.byteLength(shortened + suffix) > 240) shortened = shortened.slice(0, -1)
  return validateArchiveName(shortened + suffix)
}

const signature = (s: fs.Stats) => `${s.dev}:${s.ino}:${s.size}:${s.mtimeMs}:${s.ctimeMs}`
export async function identicalFiles(src: string, dst: string): Promise<boolean> {
  const before = [fs.statSync(src), fs.statSync(dst)]
  if (before.some(s => !s.isFile()) || before[0].size !== before[1].size) return false
  const hash = async (p: string) => {
    const h = createHash('sha256')
    for await (const chunk of fs.createReadStream(p)) h.update(chunk)
    return h.digest('hex')
  }
  const [a, b] = await Promise.all([hash(src), hash(dst)])
  if (signature(before[0]) !== signature(fs.statSync(src)) || signature(before[1]) !== signature(fs.statSync(dst))) throw new Error('校验期间文件发生变化，请重新核对')
  return a === b
}

/** Reserve the new path exclusively. Never replace an existing file, including during recovery. */
export function moveExclusive(src: string, dst: string): void {
  fs.mkdirSync(path.dirname(dst), { recursive: true })
  const before = fs.statSync(src)
  if (!before.isFile()) throw new Error('原路径不是文件')
  try { fs.linkSync(src, dst) }
  catch (e) {
    if (!['EXDEV', 'EPERM', 'EOPNOTSUPP', 'ENOTSUP'].includes((e as NodeJS.ErrnoException).code || '')) throw e
    fs.copyFileSync(src, dst, fs.constants.COPYFILE_EXCL)
  }
  try {
    // Creating a hard link changes ctime, so compare content-relevant fields here.
    const after = fs.statSync(src)
    if (after.ino !== before.ino || after.size !== before.size || after.mtimeMs !== before.mtimeMs) throw new Error('移动期间原文件发生变化')
    fs.unlinkSync(src)
  } catch (e) { fs.unlinkSync(dst); throw e }
}

/** Same guarantees as moveExclusive, without blocking the web server during a copy. */
export async function moveExclusiveAsync(src:string,dst:string):Promise<void> {
  await fs.promises.mkdir(path.dirname(dst),{recursive:true})
  const before=await fs.promises.stat(src)
  if(!before.isFile())throw new Error('原路径不是文件')
  try{await fs.promises.link(src,dst)}catch(error){
    if(!['EXDEV','EPERM','EOPNOTSUPP','ENOTSUP'].includes((error as NodeJS.ErrnoException).code||''))throw error
    await fs.promises.copyFile(src,dst,fs.constants.COPYFILE_EXCL)
  }
  try{
    const after=await fs.promises.stat(src)
    if(after.ino!==before.ino||after.size!==before.size||after.mtimeMs!==before.mtimeMs)throw new Error('移动期间原文件发生变化')
    await fs.promises.unlink(src)
  }catch(error){await fs.promises.unlink(dst);throw error}
}
