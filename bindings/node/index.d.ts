export interface ConvertOptions {
  output_dir?: string;
  config?: Record<string, unknown>;
  llm?: boolean;
  ocr?: boolean;
  screenshot?: boolean;
  alt?: boolean;
  desc?: boolean;
  profile?: 'rag' | 'obsidian' | 'okf';
}

export interface ConversionUsage {
  cost_usd: number;
  requests: number;
  input_tokens: number;
  output_tokens: number;
  by_model: Record<string, unknown>;
}

export interface ConversionOutput {
  source: string;
  markdown: string;
  llm_markdown: string | null;
  frontmatter: Record<string, unknown>;
  output_path: string | null;
  llm_output_path: string | null;
  assets: string[];
  screenshots: string[];
  images: Record<string, unknown>[];
  usage: ConversionUsage;
  skip_reason: string | null;
  duration: number;
  warnings: string[];
}

export class ConversionError extends Error {
  constructor(message: string, code: string);
  readonly code: string;
}

export const version: string;
export function convert(source: string, options?: ConvertOptions): Promise<ConversionOutput>;
export function convertSync(source: string, options?: ConvertOptions): ConversionOutput;
