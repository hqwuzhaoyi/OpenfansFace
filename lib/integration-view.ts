/** Group by the resolved source directory; a video ID is only a fallback for missing sources. */
export function groupSourceFolders<T extends { vid: string; src?: string | null }>(rows: T[]) {
  const folders = new Map<string, T[]>()
  for (const row of rows) {
    const source = (row.src || row.vid).replace(/\\/g, '/')
    const separator = source.lastIndexOf('/')
    const directory = separator < 0 ? '未记录目录' : source.slice(0, separator) || '/'
    const group = folders.get(directory)
    if (group) group.push(row)
    else folders.set(directory, [row])
  }
  return [...folders].map(([path, items]) => ({ path, items }))
}

/** A folder/filter checkbox changes only its displayed items, preserving other selections. */
export function toggleArchiveSelection(selected: Set<string>, ids: string[]) {
  const next = new Set(selected)
  const remove = ids.every(id => selected.has(id))
  for (const id of ids) {
    if (remove) next.delete(id)
    else next.add(id)
  }
  return next
}
