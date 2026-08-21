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
   * deadline. A Canvas accepts exactly one entry-script operation.
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
   * one-shot entry-script operation; a repeated call rejects before fetch or
   * stylesheet mounting.
   */
  loadLynxXml(url: string | URL): Promise<void>

  /** Registers all font faces in an OpenType font container. */
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
