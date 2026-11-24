import { invoke, isTauri } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'

export type Format = 'jpeg' | 'png' | 'webp'
export type Recipe = {
  version: 1
  output_format: Format
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
  name: string; bytes: number; mime: string | null; raster_format: Format | null
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

export function size(value: number): string {
  if (value < 1000) return value + ' B'
  if (value < 1_000_000) return (value / 1000).toLocaleString('en', { maximumFractionDigits: 1 }) + ' KB'
  if (value < 1_000_000_000) return (value / 1_000_000).toLocaleString('en', { maximumFractionDigits: 2 }) + ' MB'
  return (value / 1_000_000_000).toLocaleString('en', { maximumFractionDigits: 2 }) + ' GB'
}

export const basename = (path: string) => path.split(/[\\/]/).filter(Boolean).at(-1) ?? path
export const label = (format: Format) => ({ jpeg: 'JPEG', png: 'PNG', webp: 'WebP' })[format]
export const errorText = (error: unknown) => error instanceof Error ? error.message : String(error)
