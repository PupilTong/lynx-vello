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
   */
  executeScript(url: string | URL): Promise<void>

  /** Reserved URL entry point; currently rejects as unsupported. */
  loadStyleSheet(url: string | URL): Promise<void>
  resize(
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<void>
}

export default function init(): Promise<void>
