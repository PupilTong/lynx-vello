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
   * Relative URLs use the embedding document's base URL. The embedded QuickJS
   * realm and the browser facade do not impose a loading, startup, or execution
   * deadline.
   */
  executeScript(url: string | URL): Promise<void>

  /**
   * Fetches an author stylesheet and mounts it on the document. Sheets
   * cascade in load order. Relative URLs use the embedding document's base
   * URL.
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
