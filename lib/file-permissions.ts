import fs from 'node:fs'
import path from 'node:path'

/** NFS access() asks the server using the mapped identity; uid=0 is not a bypass. */
export function requireWritableDirectory(directory:string, purpose:string) {
  let existing=directory
  while (true) {
    try { fs.statSync(existing); break }
    catch(e) {
      if((e as NodeJS.ErrnoException).code!=='ENOENT'||path.dirname(existing)===existing)throw e
      existing=path.dirname(existing)
    }
  }
  try {fs.accessSync(existing,fs.constants.W_OK|fs.constants.X_OK)}
  catch(e){throw Object.assign(new Error(`${purpose}不可写：${existing}；请为媒体服务用户配置目录权限（${(e as NodeJS.ErrnoException).code||'未知错误'}）`),{status:403})}
}
export function requireMovePermissions(src:string,dst:string) {
  try {fs.accessSync(src,fs.constants.R_OK)}
  catch(e){throw Object.assign(new Error(`原文件不可读：${src}（${(e as NodeJS.ErrnoException).code}）`),{status:403})}
  requireWritableDirectory(path.dirname(src),'源目录')
  requireWritableDirectory(path.dirname(dst),'目标目录')
}
