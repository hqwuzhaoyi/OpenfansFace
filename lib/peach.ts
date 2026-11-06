import Docker from "dockerode"
import fs from "fs"
import path from "path"
import { execFile } from "child_process"
import { promisify } from "util"
import { pipeline } from "stream/promises"

const execFileP = promisify(execFile)
let _d: Docker | null = null
function docker() {
  if (!_d) _d = new Docker({ socketPath: process.env.DOCKER_SOCK || "/var/run/docker.sock" })
  return _d
}

/** Copy peach:/data/database/ledger.db out as <dest> (tar stream -> extract). */
export async function copyPeachLedger(dest: string): Promise<void> {
  const c = docker().getContainer(process.env.PEACH_CONTAINER || "peach")
  // Other routes/processes may refresh at the same time. Each extraction owns its files.
  const dir = await fs.promises.mkdtemp(path.join(path.dirname(dest), '.ledger-copy-'))
  try {
    const stream = (await c.getArchive({ path: "/data/database/ledger.db" })) as NodeJS.ReadableStream
    const tmp = path.join(dir, 'ledger.tar')
    await pipeline(stream, fs.createWriteStream(tmp))
    await execFileP("tar", ["-xf", tmp, "-C", dir])
    await fs.promises.rename(path.join(dir, 'ledger.db'), dest)
  } finally { await fs.promises.rm(dir, {recursive:true,force:true}) }
}

/** Run python inside peach, return demultiplexed stdout. python3 -c argv（不经 stdin，杜绝空输入静默 rc0）。 */
export async function peachExec(script: string, timeoutMs = 600000): Promise<{ out: string; code: number; err: string }> {
  const c = docker().getContainer(process.env.PEACH_CONTAINER || "peach")
  const exec = await c.exec({ Cmd: ["python3", "-c", script], AttachStdout: true, AttachStderr: true })
  const outChunks: Buffer[] = []
  const errChunks: Buffer[] = []
  const stdout = new (await import("stream")).PassThrough()
  const stderr = new (await import("stream")).PassThrough()
  stdout.on("data", (d: Buffer) => outChunks.push(d))
  stderr.on("data", (d: Buffer) => errChunks.push(d))
  const stream = (await exec.start({ hijack: false, stdin: false })) as NodeJS.ReadableStream
  docker().modem.demuxStream(stream, stdout, stderr)
  await new Promise<void>((res) => {
    stream.on("end", () => res())
    stream.on("close", () => res())
  })
  const { ExitCode } = await exec.inspect()
  return {
    out: Buffer.concat(outChunks).toString("utf8").trim(),
    code: ExitCode ?? -1,
    err: Buffer.concat(errChunks).toString("utf8").trim().slice(-300),
  }
}
