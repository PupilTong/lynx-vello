export interface PageConfig {
  defaultDisplayLinear: boolean
  defaultOverflowVisible: boolean
  enableCSSSelector: boolean
}

/** web-core raw-loader defaults; spread this object to override individual values. */
export declare const LYNX_XML_PAGE_CONFIG: Readonly<PageConfig>

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
   * deadline. The current native view accepts exactly one entry-script
   * operation; `reset()` installs a fresh view.
   */
  executeScript(url: string | URL): Promise<void>

  /**
   * Fetches an author stylesheet and mounts it on the document. Sheets
   * cascade in load order. Relative URLs use the embedding document's base
   * URL.
   */
  loadStyleSheet(url: string | URL): Promise<void>

  /**
   * Fetches and parses a Lynx XML source envelope, mounts its optional
   * stylesheet, and then runs its main-thread script. The Promise resolves
   * after the script boot sequence finishes. A background-thread script is
   * retained but not executed and produces a console warning. This is a
   * one-shot entry-script operation for the current native view; a repeated
   * call rejects before fetch or stylesheet mounting unless `reset()` ran
   * first.
   */
  loadLynxXml(url: string | URL): Promise<void>

  /**
   * Drops and rebuilds the native Lynx view while retaining the Render Worker,
   * transferred canvas, Wasm instance, page configuration, current metrics,
   * registered font containers, and selected default font family.
   */
  reset(): Promise<void>

  /** Registers all font faces and restores them after each reset. */
  registerFonts(data: ArrayBuffer | Uint8Array): Promise<number>

  /** Maps CSS system-ui, sans-serif, and serif to a registered family. */
  setDefaultFontFamily(family: string): Promise<boolean>
  resize(
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<void>
}

export default function init(): Promise<void>
