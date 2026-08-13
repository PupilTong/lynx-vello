export interface ThreadReport {
  readonly checksum: number
  readonly threads: number
}

export declare class BobcatCanvas {
  private constructor()

  static create(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<BobcatCanvas>

  addAuthorStylesheet(css: string): void
  appendElement(parent: number, child: number): number
  createPage(componentId: string, componentCssId: number): number
  createView(parentComponentUniqueId: number): number
  dropElement(element: number): void
  flushElementTree(): void
  registerFonts(bytes: Uint8Array): number
  renderIfRequested(): boolean
  resize(width: number, height: number, devicePixelRatio: number): void
}

export declare function parallelChecksum(
  bytes: Uint8Array,
  threads: number,
): Promise<ThreadReport>

export default function init(): Promise<void>
