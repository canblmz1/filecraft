import { invoke, isTauri } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'

export type Format = 'jpeg' | 'png' | 'webp' | 'pdf' | 'mp4' | 'mov' | 'heic'
export type OutputMode = Extract<Format, 'jpeg' | 'png' | 'webp'> | 'mixed'
export type Recipe = {
  version: 1
  output_format: OutputMode
  max_file_bytes: number | null
  max_batch_bytes: number | null
  min_width: number | null
  min_height: number | null
  jpeg_quality: number | null
  min_psnr_db: number | null
  lossless_only: boolean
  max_seconds: number | null
}
export type Inspection = {
  name: string; bytes: number; mime: string | null; format: Format | null; raster_format: Format | null
  width: number | null; height: number | null; exif: boolean | null; icc: boolean | null
  alpha_channel: boolean | null; animated: boolean | null; sha256: string; warnings: string[]
}
export type Input = { path: string; file: Inspection | null; error: string | null }
export type FileReport = {
  input_name: string; input_format: Format; input_sha256: string; input_bytes: number
  output_path: string; output_format: Format; output_bytes: number; output_sha256: string
  width: number; height: number; duration_ms: number; jpeg_quality: number | null
  psnr_db: number | null; pixels_identical: boolean; orientation_applied: boolean
  metadata_removed: string[]; verified_checks: string[]; unsupported_checks: string[]
  unchanged?: boolean; pdf_pages?: number | null; video_duration_seconds?: number | null; video_crf?: number | null
  video_tone_mapped?: boolean; icc_profile_preserved?: boolean
  pdf_images?: { images_reencoded: number; max_dpi: number; jpeg_quality: number; min_psnr_db: number } | null
}
export type EngineStatus = {
  available: boolean; source: 'managed' | 'path' | 'environment' | null; ffmpeg: string | null; version: string | null
  hdr_tone_mapping: boolean; heic: boolean; managed_directory: string | null; installable: boolean; install_bytes: number | null
}
export type EngineInstall = {
  running: boolean; error: string | null; status: EngineStatus | null
  progress: { stage: 'downloading' | 'verifying' | 'extracting' | 'testing'; received_bytes: number; total_bytes: number } | null
}
export type Report = {
  status: 'completed' | 'partial' | 'failed' | 'cancelled'
  input_bytes: number; output_bytes: number; goal: Recipe; duration_ms: number
  files: FileReport[]; error: string | null; error_code: string | null
}
export type Job = {
  id: string; created_ms: number; count: number; error: string | null; report: Report | null
  state: Report['status'] | 'pending' | 'inspecting' | 'planning' | 'running' | 'verifying'
  progress: {
    files_finished: number; files_total: number; file_index: number | null
    candidates_finished: number; candidates_total: number
    steps_finished: number; steps_total: number
  } | null
}
export type Capability = {
  id: string; version: string; interface_version: number; available: boolean; license: string
  input_formats: Format[]; output_formats: Format[]; operations: string[]
  external_dependencies: string[]; constraints: string[]; validation: string[]
  max_pixels: number; max_input_bytes: number; cancellation: string
}

export const desktop = isTauri()
const json = [{ name: 'JSON', extensions: ['json'] }]
export const api = {
  inputPicker: () => open({ multiple: true, title: 'Add files to FileCraft' }),
  folderPicker: () => open({ directory: true, title: 'Choose output folder' }),
  inspect: (paths: string[]) => invoke<Input[]>('inspect_files', { paths }),
  start: (paths: string[], destination: string, recipe: Recipe) => invoke<Job>('start_job', { paths, destination, recipe }),
  job: (id: string) => invoke<Job>('job_status', { id }),
  cancel: (id: string) => invoke<void>('cancel_job', { id }),
  history: () => invoke<Job[]>('job_history'),
  capabilities: () => invoke<Capability[]>('engine_capabilities'),
  videoEngine: () => invoke<EngineStatus>('video_engine_status'),
  installEngine: () => invoke<void>('install_engine'),
  installState: () => invoke<EngineInstall>('engine_install_state'),
  cancelInstall: () => invoke<void>('cancel_engine_install'),
  async importRecipe() {
    const path = await open({ filters: json, title: 'Import a FileCraft recipe' })
    return path ? invoke<Recipe>('import_recipe', { path }) : null
  },
  async exportRecipe(recipe: Recipe) {
    const path = await save({ filters: json, defaultPath: 'filecraft-recipe.json' })
    if (path) await invoke<void>('export_recipe', { path, recipe })
    return Boolean(path)
  },
  async exportReport(id: string) {
    const path = await save({ filters: json, defaultPath: 'filecraft-report.json' })
    if (path) await invoke<void>('export_report', { path, id })
    return Boolean(path)
  },
}

export const active = (job: Job) => !['completed', 'partial', 'failed', 'cancelled'].includes(job.state)
export const baseRecipe: Recipe = {
  version: 1, output_format: 'webp', max_file_bytes: null, max_batch_bytes: null,
  min_width: null, min_height: null, jpeg_quality: null, min_psnr_db: null,
  lossless_only: true, max_seconds: null,
}
export const mixedRecipe: Recipe = {
  ...baseRecipe, output_format: 'mixed', max_batch_bytes: 20_000_000, lossless_only: false,
}

export function size(value: number): string {
  if (value < 1000) return value + ' B'
  if (value < 1_000_000) return (value / 1000).toLocaleString('en', { maximumFractionDigits: 1 }) + ' KB'
  if (value < 1_000_000_000) return (value / 1_000_000).toLocaleString('en', { maximumFractionDigits: 2 }) + ' MB'
  return (value / 1_000_000_000).toLocaleString('en', { maximumFractionDigits: 2 }) + ' GB'
}

export const basename = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path
export const label = (format: Format | OutputMode) => ({
  jpeg: 'JPEG', png: 'PNG', webp: 'WebP', pdf: 'PDF', mp4: 'MP4', mov: 'MOV', heic: 'HEIC', mixed: 'Mixed batch',
})[format]
export const needsVideoEngine = (format: Format | null | undefined) => format === 'mp4' || format === 'mov' || format === 'heic'

/** True when an available engine can take `format` toward the goal. */
export function accepts(engines: Capability[], format: Format | null | undefined, goal: OutputMode) {
  return !!format && engines.some(engine => engine.available && engine.input_formats.includes(format)
    && (goal === 'mixed' || engine.output_formats.includes(goal)))
}
export const errorText = (error: unknown) => error instanceof Error ? error.message : String(error)
