export declare class BobcatCanvas {
  private constructor()

  readonly error: Error | undefined
  onerror: ((error: Error) => void) | null

  static create(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<BobcatCanvas>

  addAuthorStylesheet(css: string): Promise<void>
  appendElement(parent: number, child: number): Promise<number>
  createPage(componentId: string, componentCssId: number): Promise<number>
  createView(parentComponentUniqueId: number): Promise<number>
  dispose(): Promise<void>
  dropElement(element: number): Promise<void>
  flushElementTree(): Promise<void>
  registerFonts(bytes: Uint8Array): Promise<number>
  resize(
    width: number,
    height: number,
    devicePixelRatio: number,
  ): Promise<void>
}

export default function init(): Promise<void>
