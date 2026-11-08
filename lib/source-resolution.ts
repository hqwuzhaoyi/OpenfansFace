/** Stable video IDs may retain an old Windows path after media has moved. */
export function sourceCandidates(vid: string, stored: string | null | undefined, mediaRoot: string): string[] {
  const root=mediaRoot.replace(/\/$/,'')
  const map=(value:string)=>{
    const normalized=value.replace(/\\/g,'/')
    if(normalized.split('/').includes('..'))return null
    if(normalized.startsWith(root+'/'))return normalized
    const marker=normalized.indexOf('/media/pikpak/')
    return marker>=0?root+'/pikpak/'+normalized.slice(marker+'/media/pikpak/'.length):null
  }
  return [...new Set([stored?map(stored):null,map(vid)].filter((v):v is string=>!!v))]
}
export function chooseSource(vid:string,stored:string|null|undefined,mediaRoot:string,isFile:(path:string)=>boolean):string|null {
  const candidates=sourceCandidates(vid,stored,mediaRoot)
  if(candidates.length<=1)return candidates[0]||null
  return candidates.find(isFile)||candidates[0]||null
}

/** creators is the already-organized library; sibling directory names do not match. */
export function creatorFolder(filePath:string,mediaRoot:string):string|null {
  const normalized=sourceCandidates(filePath,null,mediaRoot)[0]
  const prefix=mediaRoot.replace(/\/$/,'')+'/pikpak/creators/'
  if(!normalized?.startsWith(prefix))return null
  return normalized.slice(prefix.length).split('/')[0]||null
}
