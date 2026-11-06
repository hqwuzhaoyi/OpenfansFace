interface Video { vid: string; size: number; conf?: string; path?: string }
interface Cluster { cid: string; vids: Video[]; [key: string]: any }

/** Review is per video; a queued subset must not hide the rest of its cluster. */
export function reviewClusters<T extends Cluster>(clusters: T[], queue: { vid: string }[]) {
  const queued = new Set(queue.map(v => v.vid))
  return clusters.map(c => {
    const vids = c.vids.filter(v => !queued.has(v.vid))
    const counts = new Map<string, number>()
    for (const v of vids) {
      const rel = (v.path||v.vid).replace(/\\/g, "/").split("/pikpak/")[1] || v.vid
      const parts = rel.split("/")
      const folder = parts.length > 2 ? parts.slice(0, 2).join("/") : parts[0]
      counts.set(folder, (counts.get(folder) || 0) + 1)
    }
    return { ...c, vids, n_videos: vids.length,
      rep_thumb: (vids[0] as any)?.thumb || c.rep_thumb,
      collections: [...counts].sort((a,b)=>b[1]-a[1]).slice(0,3).map(([name,n])=>({name,n})),
      total_gb: +(vids.reduce((n, v) => n + v.size, 0) / 2 ** 30).toFixed(1),
      weak_count: vids.filter(v => v.conf === 'weak').length }
  }).filter(c => c.vids.length > 0)
}
