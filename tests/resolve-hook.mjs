import { pathToFileURL } from 'node:url'
import path from 'node:path'

const rootDir = process.cwd()

export async function resolve(specifier, context, nextResolve) {
  if (specifier === 'next/server') {
    return nextResolve('next/server.js', context)
  }
  if (specifier.startsWith('@/')) {
    const rel = specifier.slice(2)
    // Check if it needs .ts extension
    const fullPath = path.resolve(rootDir, rel)
    return nextResolve(pathToFileURL(fullPath).href + '.ts', context)
  }
  return nextResolve(specifier, context)
}
