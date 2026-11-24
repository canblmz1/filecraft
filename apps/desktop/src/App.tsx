import { useCallback, useEffect, useState, type FormEvent } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  ArrowDownToLine, ArrowRight, ArrowUpRight, Boxes, Check, ChevronDown, CircleAlert,
  Clock3, FileImage, FileSearch, FileText, Folder, HardDrive, History, Image, Layers2, LockKeyhole,
  Plus, ShieldCheck, SlidersHorizontal, Sparkles, Upload, Video, Workflow, X,
} from 'lucide-react'
import { active, api, baseRecipe, basename, desktop, errorText, label, mixedRecipe, size, type Capability, type Input, type Job, type OutputMode, type Recipe } from './api'

type View = 'Workbench' | 'Inspector' | 'Recipes' | 'History' | 'Engines'
type Attempt = { paths: string[]; folder: string; recipe: Recipe }
const nav = [
  { title: 'Workbench', icon: Layers2 },
  { title: 'Inspector', icon: FileSearch },
  { title: 'Recipes', icon: Workflow },
  { title: 'History', icon: History },
  { title: 'Engines', icon: Boxes },
] as const

const presets = [
  { name: 'Web-ready images', description: 'Lossless WebP copies', icon: Image, tone: 'orange', recipe: baseRecipe },
  { name: 'Application package', description: 'Mixed files within 20 MB', icon: ArrowDownToLine, tone: 'blue', recipe: mixedRecipe },
  { name: 'Clean copies', description: 'PNG re-encode with no EXIF', icon: ShieldCheck, tone: 'sage', recipe: { ...baseRecipe, output_format: 'png' } as Recipe },
] as const

const modes: OutputMode[] = ['mixed', 'webp', 'jpeg', 'png']

function Brand({ onClick }: { onClick: () => void }) {
  return <button className="brand" type="button" onClick={onClick}>
    <span className="brand-mark" aria-hidden="true"><Layers2 size={22} strokeWidth={2.4} /></span>
    <span>FileCraft<small>PRIVATE WORKBENCH</small></span>
  </button>
}

function DropZone({ count, loading, dragging, pick, videoAvailable }: {
  count: number; loading: boolean; dragging: boolean; pick: () => void; videoAvailable: boolean
}) {
  return <div className={'dropzone' + (dragging ? ' is-dragging' : '') + (count ? ' compact' : '')}>
    <div className="drop-art" aria-hidden="true">
      <span className="orbit orbit-one" /><span className="orbit orbit-two" />
      <span className="art-card art-back"><Image size={29} strokeWidth={1.5} /></span>
      <span className="art-card art-front"><FileImage size={35} strokeWidth={1.45} /></span>
      <span className="art-plus"><Plus size={18} strokeWidth={2.5} /></span>
      <span className="art-spark one">✦</span><span className="art-spark two">✧</span>
    </div>
    <h3>{count ? 'Add more files' : 'Drop your files here'}</h3>
    <p>{loading ? 'Inspecting files locally…' : 'Drag and drop files, or browse your device.'}</p>
    <button className="button primary browse" onClick={pick}><Plus size={17} /> Browse files</button>
    <span className="drop-note">JPEG <i /> PNG <i /> WebP <i /> PDF {videoAvailable && <><i /> MP4</>}</span>
  </div>
}

function InputList({ rows, remove }: { rows: Input[]; remove: (path: string) => void }) {
  if (!rows.length) return null
  const matches = new Map<string, number>()
  for (const { file } of rows) {
    if (file) matches.set(file.sha256, (matches.get(file.sha256) ?? 0) + 1)
  }
  return <ul className="file-list">{rows.map(({ path, file, error }) => <li key={path}>
    <div className="file-row">
      <span className="square orange">{file?.format === 'pdf' ? <FileText size={19} /> : file?.format === 'mp4' ? <Video size={19} /> : <FileImage size={19} />}</span>
      <div className="file-main"><strong title={path}>{file?.name ?? basename(path)}</strong>
        <small>{file ? [file.mime ?? 'Unknown signature', size(file.bytes), file.width ? file.width + ' × ' + file.height + ' px' : null].filter(Boolean).join(' · ') : error}</small>
      </div>
      <button className="icon-button" onClick={() => remove(path)} aria-label={'Remove ' + basename(path)}><X size={17} /></button>
    </div>
    {file && <details className="details"><summary>Inspection details</summary>
      <p>SHA-256 <code>{file.sha256}</code></p>
      {(matches.get(file.sha256) ?? 0) > 1 && <p>This fingerprint matches another input.</p>}
      {file.raster_format && <p>EXIF {flag(file.exif)} · ICC {flag(file.icc)} · Alpha channel {flag(file.alpha_channel)} · Animated {flag(file.animated)}</p>}
      {file.warnings.map(warning => <p className="warning" key={warning}>{warning}</p>)}
    </details>}
  </li>)}</ul>
}

const flag = (value: boolean | null) => value === null ? 'unavailable' : value ? 'detected' : 'undetected'
type NumberField = Exclude<keyof Recipe, 'version' | 'output_format' | 'lossless_only'>

function GoalPanel({ recipe, change, output, choose, submit, available, enabled, videoAvailable }: {
  recipe: Recipe; change: (value: Recipe) => void; output: string; choose: () => void
  submit: (event: FormEvent) => void; available: OutputMode[]; enabled: boolean; videoAvailable: boolean
}) {
  function numberField(key: NumberField, title: string, unit: string, max?: number, min = 1) {
    return <label className="field" key={key}>{title}<span className="input-wrap">
      <input type="number" min={min} max={max} placeholder="Optional" value={recipe[key] ?? ''}
        onChange={event => change({ ...recipe, [key]: event.target.value === '' ? null : event.target.valueAsNumber })} />
      <span>{unit}</span>
    </span></label>
  }
  function sizeField(key: 'max_batch_bytes' | 'max_file_bytes', title: string) {
    return <label className="field">{title}<span className="input-wrap">
      <input type="number" min={0.000001} step="0.000001" placeholder={key === 'max_batch_bytes' && recipe.output_format === 'mixed' ? 'Required' : 'Optional'}
        required={key === 'max_batch_bytes' && recipe.output_format === 'mixed'}
        value={recipe[key] === null ? '' : recipe[key] / 1_000_000}
        onChange={event => change({ ...recipe, [key]: event.target.value === '' ? null : Math.round(event.target.valueAsNumber * 1_000_000) })} />
      <span>MB</span>
    </span></label>
  }
  return <form className="goal-panel" onSubmit={submit}>
    <div className="panel-heading"><span className="square orange"><SlidersHorizontal size={19} /></span><div><h2>Set a goal</h2><p>Define what the output should meet.</p></div></div>
    <fieldset className="formats"><legend>Output mode</legend>
      {available.map(format => <button key={format} type="button" className={recipe.output_format === format ? 'selected' : ''} aria-pressed={recipe.output_format === format}
        onClick={() => change({ ...recipe, output_format: format, max_batch_bytes: format === 'mixed' ? recipe.max_batch_bytes ?? mixedRecipe.max_batch_bytes : recipe.max_batch_bytes, lossless_only: format === 'webp' || format === 'png', jpeg_quality: format === 'jpeg' ? 90 : null, min_psnr_db: null })}>{label(format)}</button>)}
    </fieldset>
    {recipe.output_format === 'mixed' && <p className="goal-hint">{videoAvailable ? 'Images, PDF and MP4 share one size limit.' : 'Images and PDF share one size limit. MP4 requires FFmpeg and FFprobe on PATH.'}</p>}
    <div className="fields">
      {sizeField('max_batch_bytes', 'Batch size limit')}
      {sizeField('max_file_bytes', 'Per-file size limit')}
    </div>
    <details className="advanced">
      <summary><SlidersHorizontal size={15} /> Advanced constraints <ChevronDown size={15} /></summary>
      <div className="fields">
        {numberField('min_width', 'Minimum width', 'px', 12000)}
        {numberField('min_height', 'Minimum height', 'px', 12000)}
        {numberField('max_seconds', 'Time budget', 'sec', 86400)}
        {recipe.output_format === 'jpeg' && numberField('jpeg_quality', 'Preferred quality', '%', 100, 35)}
        {recipe.output_format === 'jpeg' && numberField('min_psnr_db', 'Minimum PSNR', 'dB', 100)}
      </div>
      {recipe.output_format === 'mixed' && <label className="lossless-toggle"><input type="checkbox" checked={recipe.lossless_only} onChange={event => change({ ...recipe, lossless_only: event.target.checked })} /> Keep MP4 bytes unchanged</label>}
    </details>
    <div className="folder-field"><span>Save to</span><button type="button" className="button folder-button" onClick={choose} title={output || undefined}><Folder size={17} /><span>{output ? basename(output) : 'Choose output folder'}</span><ChevronDown size={15} /></button></div>
    <button className="button primary run" type="submit" disabled={!enabled}><Sparkles size={17} /> Run &amp; verify <ArrowRight size={17} /></button>
    <p className="safe"><LockKeyhole size={13} /> Originals are kept intact</p>
  </form>
}

function Presets({ select }: { select: (recipe: Recipe) => void }) {
  return <div className="preset-grid">{presets.map(({ name, description, icon: Icon, tone, recipe }) => <button className="preset" key={name} onClick={() => select({ ...recipe })}>
    <span className={'square ' + tone}><Icon size={19} /></span><ArrowUpRight className="shortcut-arrow" size={16} />
    <strong>{name}</strong><small>{description}</small>
  </button>)}</div>
}

function JobCard({ job, cancel, retry, save, busy }: {
  job: Job; cancel: () => void; retry?: () => void; save: () => void; busy?: boolean
}) {
  const report = job.report
  const changes = report ? report.input_bytes - report.output_bytes : 0
  return <article className="job-card">
    <div className="section-head"><div className="panel-heading"><span className={'square ' + (job.state === 'completed' ? 'sage' : 'orange')}>
      {job.state === 'completed' ? <Check size={19} /> : active(job) ? <Clock3 size={19} /> : <CircleAlert size={19} />}
    </span><div><h3>{job.state === 'completed' ? 'Verified output' : job.state[0].toUpperCase() + job.state.slice(1)}</h3>
      <p>{job.count} {job.count === 1 ? 'file' : 'files'} · {new Date(job.created_ms).toLocaleString()}</p></div></div>
      {active(job) ? <button className="button secondary" onClick={cancel} disabled={busy}>{busy ? 'Cancelling…' : 'Cancel job'}</button> :
        <div className="actions">{retry && <button className="button secondary" onClick={retry}>Retry</button>}{report && <button className="button secondary" onClick={save}><ArrowDownToLine size={15} /> Report</button>}</div>}
    </div>
    {job.progress && active(job) && <div className="progress">
      <progress aria-label="Overall batch progress" max={job.progress.steps_total} value={job.progress.steps_finished} />
      <p>{job.progress.file_index === null
        ? job.progress.files_finished + ' / ' + job.progress.files_total + ' files in this stage'
        : 'File ' + (job.progress.file_index + 1) + ' of ' + job.progress.files_total + ' · candidate ' + Math.min(job.progress.candidates_finished + 1, job.progress.candidates_total) + '/' + job.progress.candidates_total}</p>
    </div>}
    {job.error && <p className="warning">{job.error}</p>}
    {report && report.files.length > 0 && <><div className="stats">
      <div><small>Original</small><strong>{size(report.input_bytes)}</strong></div>
      <div><small>Output</small><strong>{size(report.output_bytes)}</strong></div>
      {job.state === 'completed' && <div><small>{changes < 0 ? 'Larger by' : 'Saved'}</small><strong>{size(Math.abs(changes))}</strong></div>}
      <div><small>Elapsed</small><strong>{(report.duration_ms / 1000).toFixed(2)} s</strong></div>
    </div>{job.state === 'completed' && !!report.goal.max_batch_bytes && <p className="goal-result"><Check size={14} /> {size(report.output_bytes)} of {size(report.goal.max_batch_bytes)} total limit</p>}
    {report.files.map(file => <details className="details output" key={file.output_path}>
      <summary><span>{basename(file.output_path)}</span><small>{size(file.input_bytes)} → {size(file.output_bytes)}</small></summary>
      <p>{[
        file.unchanged ? 'Byte-for-byte copy' : label(file.output_format),
        file.width > 0 ? file.width + ' × ' + file.height + ' px' : null,
        file.pdf_pages ? file.pdf_pages + ' pages' : null,
        file.video_duration_seconds ? file.video_duration_seconds.toFixed(2) + ' s' : null,
        file.video_crf ? 'CRF ' + file.video_crf : null,
        file.pixels_identical && !file.unchanged ? 'Identical decoded pixels' : null,
        file.psnr_db != null ? 'PSNR ' + file.psnr_db.toFixed(2) + ' dB' : null,
      ].filter(Boolean).join(' · ')}</p>
      <p>{file.verified_checks.join(' · ')}</p>
      {file.orientation_applied && <p>EXIF orientation applied.</p>}
      {file.metadata_removed.length > 0 && <p>Detected metadata removed {file.metadata_removed.join(', ')}.</p>}
      <p>Unchecked {file.unsupported_checks.join(', ')}.</p>
      <p>Output SHA-256 <code>{file.output_sha256}</code></p>
      <p>Saved at <code>{file.output_path}</code></p>
    </details>)}</>}
  </article>
}

const intro = {
  Workbench: ['Your file workbench', 'Inspect your files, set an output goal, and verify the results.'],
  Inspector: ['File inspector', 'Read file signatures, dimensions, and fingerprints on your device.'],
  Recipes: ['Recipes', 'Reuse and share structured processing goals as JSON.'],
  History: ['Job history', 'Review local job status and export verification reports.'],
  Engines: ['Processing engines', 'See the formats and checks available on this device.'],
}

export default function App() {
  const [view, setView] = useState<View>('Workbench')
  const [rows, setRows] = useState<Input[]>([])
  const [goal, setGoal] = useState<Recipe>(mixedRecipe)
  const [destination, setDestination] = useState('')
  const [loading, setLoading] = useState(0)
  const [dragging, setDragging] = useState(false)
  const [job, setJob] = useState<Job>()
  const [lastAttempt, setLastAttempt] = useState<Attempt>()
  const [history, setHistory] = useState<Job[]>([])
  const [engines, setEngines] = useState<Capability[]>([])
  const [feedback, setFeedback] = useState<{ text: string; error: boolean }>()
  const [submitting, setSubmitting] = useState(false)
  const [cancelling, setCancelling] = useState(false)
  const runningId = job && active(job) ? job.id : undefined
  const raster = engines.find(engine => engine.id === 'native-raster' && engine.available)
  const pdf = engines.some(engine => engine.id === 'native-pdf' && engine.available)
  const videoAvailable = engines.some(engine => engine.id === 'ffmpeg-video' && engine.available)
  const offered = desktop
    ? modes.filter(mode => mode === 'mixed' ? raster && pdf : raster?.output_formats.includes(mode))
    : modes
  const unsupported = rows.find(({ file }) => !file?.format || (
    goal.output_format === 'mixed'
      ? !file.raster_format && !(file.format === 'pdf' && pdf) && !(file.format === 'mp4' && videoAvailable)
      : !file.raster_format
  ))

  const fail = useCallback((error: unknown) => setFeedback({ text: errorText(error), error: true }), [])
  const add = useCallback(async (paths: string[]) => {
    setLoading(count => count + 1)
    try {
      const inspected = await api.inspect([...new Set(paths)])
      setRows(existing => Array.from(new Map([...existing, ...inspected].map(row => [row.path, row])).values()))
    } catch (error) { fail(error) }
    finally { setLoading(count => count - 1) }
  }, [fail])

  useEffect(() => {
    if (!desktop) return
    let disposed = false
    const listener = getCurrentWebview().onDragDropEvent(event => {
      if (disposed) return
      const payload = event.payload
      setDragging(payload.type === 'enter' || payload.type === 'over')
      if (payload.type === 'drop') void add(payload.paths)
    })
    listener.catch(fail)
    api.capabilities().then(available => { if (!disposed) setEngines(available) }, fail)
    return () => { disposed = true; void listener.then(unlisten => unlisten(), () => undefined) }
  }, [add, fail])

  useEffect(() => {
    if (!desktop || (view !== 'History' && !runningId)) return
    let disposed = false
    let timeout: ReturnType<typeof setTimeout>
    async function poll() {
      try {
        if (runningId) {
          const current = await api.job(runningId)
          if (!disposed) setJob(current)
        }
        if (view === 'History') {
          const records = await api.history()
          if (!disposed) setHistory(records)
        }
      } catch (error) { if (!disposed) fail(error) }
      if (!disposed) timeout = setTimeout(poll, 450)
    }
    void poll()
    return () => { disposed = true; clearTimeout(timeout) }
  }, [view, runningId, fail])

  function notice(text: string) { setFeedback({ text, error: false }) }
  async function perform(action: () => Promise<void>) {
    if (!desktop) { fail('Open the FileCraft desktop app to use local file actions.'); return }
    try { await action() } catch (error) { fail(error) }
  }
  const pick = () => void perform(async () => { const files = await api.inputPicker(); if (files?.length) await add(files) })
  const chooseFolder = () => void perform(async () => { const path = await api.folderPicker(); if (path) setDestination(path) })
  const importGoal = () => void perform(async () => { const recipe = await api.importRecipe(); if (recipe) { setGoal(recipe); setView('Workbench'); notice('Recipe loaded. Review its goals before running.') } })
  const saveGoal = () => void perform(async () => { if (await api.exportRecipe(goal)) notice('Recipe saved.') })
  const saveReport = (id: string) => void perform(async () => { if (await api.exportReport(id)) notice('Verification report saved.') })
  const remove = (path: string) => setRows(existing => existing.filter(row => row.path !== path))
  const selectPreset = (recipe: Recipe) => { setGoal(recipe); setView('Workbench'); notice('Recipe applied to the current workbench.') }

  const canRun = rows.length > 0 && !unsupported && !loading && offered.includes(goal.output_format) && !submitting && !runningId
  async function start(event?: FormEvent, retry?: Attempt) {
    event?.preventDefault()
    if (submitting || runningId || (!retry && !canRun)) return
    setSubmitting(true)
    await perform(async () => {
      const folder = retry?.folder || destination || await api.folderPicker()
      if (!folder) return
      const attempt = retry ?? { paths: rows.map(row => row.path), folder, recipe: goal }
      setDestination(folder)
      setJob(await api.start(attempt.paths, attempt.folder, attempt.recipe))
      setLastAttempt(attempt)
      setCancelling(false)
      setFeedback(undefined)
    })
    setSubmitting(false)
  }
  function cancel(id: string) {
    setCancelling(true)
    void perform(async () => {
      try { await api.cancel(id) }
      catch (error) { setCancelling(false); throw error }
    })
  }

  return <div className="shell">
    <aside className="sidebar">
      <Brand onClick={() => setView('Workbench')} />
      <div className="nav-caption">WORKSPACE</div>
      <nav aria-label="Workspace">{nav.map(({ title, icon: Icon }) => <button key={title} aria-label={title} className={view === title ? 'current' : ''} aria-current={view === title ? 'page' : undefined} onClick={() => setView(title)}><Icon size={18} strokeWidth={1.8} /><span>{title}</span>{view === title && <span className="nav-dot" />}</button>)}</nav>
      <div className="sidebar-status"><span className="status-icon"><HardDrive size={19} /></span><div><strong>Local workspace</strong><small>Runs on your device</small></div></div>
      <div className="version"><span>v0.0.1</span><span>OPEN SOURCE</span></div>
    </aside>
    <div className="application">
      <header className="toolbar"><div className="breadcrumb"><Layers2 size={16} /><span>Workspace</span><span className="slash">/</span><strong>{view}</strong></div><div className="actions">
        <button className="button secondary" onClick={importGoal}><Upload size={15} /> Import recipe</button>
      </div></header>
      <main id="content">
        <div className="page-head"><div className="page-tag"><span /> {view === 'Workbench' ? 'FILE WORKSPACE' : view.toUpperCase()}</div><h1>{intro[view][0]}<span>.</span></h1><p>{intro[view][1]}</p></div>
        {!desktop && <p className="preview-note">Browser preview · File operations are available in the desktop app.</p>}
        {feedback && <div className={'notice ' + (feedback.error ? 'error' : '')} role={feedback.error ? 'alert' : 'status'}>{feedback.error ? <CircleAlert size={16} /> : <Check size={16} />}<span>{feedback.text}</span><button className="icon-button" onClick={() => setFeedback(undefined)} aria-label="Dismiss message"><X size={16} /></button></div>}
        {view === 'Workbench' && <div className="workspace-grid">
          <div className="main-column">
            <section aria-label="Input files">
              <div className="section-head"><h2>Input files</h2><span className="file-count">{rows.length} {rows.length === 1 ? 'file' : 'files'}{rows.length > 0 ? ' · ' + size(rows.reduce((total, row) => total + (row.file?.bytes ?? 0), 0)) : ''}</span></div>
              <DropZone count={rows.length} loading={loading > 0} dragging={dragging} pick={pick} videoAvailable={videoAvailable} />
              <InputList rows={rows} remove={remove} />
              {unsupported && <p className="warning">{unsupported.file?.format === 'mp4' && !videoAvailable
                ? 'MP4 requires FFmpeg with libx264 and AAC plus FFprobe on PATH. Restart FileCraft after installing them.'
                : goal.output_format !== 'mixed' && (unsupported.file?.format === 'pdf' || unsupported.file?.format === 'mp4')
                  ? 'Choose Mixed batch to process PDF or MP4 with your images.'
                  : `Remove ${unsupported.file?.name ?? basename(unsupported.path)} to run this goal.`}</p>}
            </section>
            {job && <JobCard job={job} cancel={() => cancel(job.id)} busy={cancelling} retry={lastAttempt && !submitting && (job.state === 'failed' || job.state === 'cancelled') ? () => void start(undefined, lastAttempt) : undefined} save={() => saveReport(job.id)} />}
            <section><div className="section-head shortcuts-title"><h2>Quick recipes</h2><button className="text-button" onClick={() => setView('Recipes')}>All recipes <ArrowRight size={14} /></button></div><Presets select={selectPreset} /></section>
          </div>
          <div><GoalPanel recipe={goal} change={setGoal} output={destination} choose={chooseFolder} submit={event => void start(event)} available={offered} enabled={canRun} videoAvailable={videoAvailable} />
            <div className="privacy-note"><ShieldCheck size={16} /><span>FileCraft processes and verifies files on this device.</span></div>
          </div>
        </div>}
        {view === 'Inspector' && <section className="main-column"><DropZone count={rows.length} loading={loading > 0} dragging={dragging} pick={pick} videoAvailable={videoAvailable} /><InputList rows={rows} remove={remove} /></section>}
        {view === 'Recipes' && <section className="main-column"><Presets select={selectPreset} /><div className="job-card section-head"><div><h3>Current workbench recipe</h3><p>Settings only. Input files and output paths stay out of the recipe.</p></div><button className="button secondary" onClick={saveGoal}><ArrowDownToLine size={16} /> Export recipe</button></div></section>}
        {view === 'History' && <section className="main-column">{history.length ? history.map(item => <JobCard key={item.id} job={item} cancel={() => cancel(item.id)} save={() => saveReport(item.id)} busy={cancelling && item.id === runningId} />) : <div className="job-card"><h3>No jobs yet</h3><p>Processing jobs appear here once submitted.</p></div>}</section>}
        {view === 'Engines' && <section className="main-column">{engines.map(engine => <article className="job-card" key={engine.id}>
          <div className="panel-heading"><span className="square sage"><Boxes size={19} /></span><div><h3>{engine.id}</h3><p>Adapter {engine.version} · {engine.available ? 'Available' : 'Unavailable'}</p></div></div>
          <p>{engine.input_formats.map(label).join(', ')} input · {engine.output_formats.map(label).join(', ')} output</p><p>{engine.constraints.join(' · ')}</p>
          <p>Input limit {size(engine.max_input_bytes)}{engine.max_pixels > 0 && ' · ' + engine.max_pixels.toLocaleString('en') + ' pixels'}</p>
          {engine.external_dependencies.length > 0 && <p>Requires {engine.external_dependencies.join(' and ')} on PATH.</p>}
          <p>{engine.license}</p><p>Checks {engine.validation.join(', ')}</p>
        </article>)}</section>}
      </main>
    </div>
  </div>
}
