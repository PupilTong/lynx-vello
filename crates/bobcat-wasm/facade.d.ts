export interface PageConfig {
  defaultDisplayLinear: boolean
  defaultOverflowVisible: boolean
  enableCSSSelector: boolean
}

/** web-core raw-loader defaults; spread this object to override individual values. */
export declare const LYNX_XML_PAGE_CONFIG: Readonly<PageConfig>

/**
 * A Worker-owned Bobcat view attached to one HTML canvas. Active
 * `pointerdown`/`pointermove`/`pointerup`/`pointercancel` sequences on the
 * canvas are captured and forwarded to the native input router automatically.
 */
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

  /** Releases pointer capture/listeners and terminates the Render Worker. */
  dispose(): Promise<void>

  /**
   * Fetches a page's author stylesheets and its main-thread entry script and
   * shows it. A native view is its page, so each load builds a fresh one and
   * drops the view before it. Stylesheets cascade in the order given and all
   * mount before the entry script runs. Resolves after the script boot
   * sequence finishes and rejects on loading or evaluation error, leaving the
   * previous page running if the fetch was what failed. Relative URLs use the
   * embedding document's base URL. Nothing imposes a deadline.
   */
  load(url: string | URL, styleSheetUrls?: readonly (string | URL)[]): Promise<void>

  /**
   * Fetches and parses a Lynx XML source envelope and shows it: the same load
   * as `load()`, with the envelope's sections as the sources. A
   * background-thread script produces a console warning and is not executed.
   */
  loadLynxXml(url: string | URL): Promise<void>

  /** Retains font faces for every page this canvas loads; call before a load. */
  registerFonts(data: ArrayBuffer | Uint8Array): Promise<void>

  /**
   * Maps CSS system-ui, sans-serif, and serif to a family for every page this
   * canvas loads. An unknown family makes the next load reject.
   */
  setDefaultFontFamily(family: string): Promise<void>
  resize(
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<void>
}

export default function init(): Promise<void>
