export interface PageConfig {
  defaultDisplayLinear: boolean
  defaultOverflowVisible: boolean
  enableCSSSelector: boolean
}

export declare class BobcatCanvas {
  private constructor()

  readonly error: Error | undefined
  onerror: ((error: Error) => void) | null

  static create(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    devicePixelRatio: number,
    pageConfig: PageConfig,
  ): Promise<BobcatCanvas>

  dispose(): Promise<void>

  /**
   * Fetches and runs the main-thread entry script. This resolves after the
   * script boot sequence finishes and rejects on loading or evaluation error.
   * Relative URLs use the embedding document's base URL. The browser VM has
   * no execution interrupt: a non-terminating script leaves this Promise
   * pending, and recovery requires disposing this canvas and creating a new
   * one. Native QuickJS embedders may provide different timeout policy.
   */
  executeScript(url: string | URL): Promise<void>

  /**
   * Reserved URL entry point; currently rejects as unsupported. Relative URLs
   * use the embedding document's base URL.
   */
  loadStyleSheet(url: string | URL): Promise<void>

  /** Registers all font faces in an OpenType font container. */
  registerFonts(data: ArrayBuffer | Uint8Array): Promise<number>
  resize(
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<void>
}

export default function init(): Promise<void>
