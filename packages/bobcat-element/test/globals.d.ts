import type * as elementPapi from "../src/element-papi.ts";

declare global {
  /** Rstest-only backing object for its `bobcat-internal:host` replacement. */
  var __bobcatTestHost: BobcatNative | undefined;

  // The PAPI bindings element-papi.test.ts copies onto `globalThis` and then
  // calls as free names.
  var __CreatePage: typeof elementPapi.__CreatePage;
  var __CreateElement: typeof elementPapi.__CreateElement;
  var __CreateWrapperElement: typeof elementPapi.__CreateWrapperElement;
  var __CreateText: typeof elementPapi.__CreateText;
  var __CreateImage: typeof elementPapi.__CreateImage;
  var __CreateView: typeof elementPapi.__CreateView;
  var __CreateScrollView: typeof elementPapi.__CreateScrollView;
  var __CreateRawText: typeof elementPapi.__CreateRawText;
  var __CreateList: typeof elementPapi.__CreateList;
  var __AppendElement: typeof elementPapi.__AppendElement;
  var __InsertElementBefore: typeof elementPapi.__InsertElementBefore;
  var __RemoveElement: typeof elementPapi.__RemoveElement;
  var __ReplaceElement: typeof elementPapi.__ReplaceElement;
  var __ReplaceElements: typeof elementPapi.__ReplaceElements;
  var __SwapElement: typeof elementPapi.__SwapElement;
  var __SetClasses: typeof elementPapi.__SetClasses;
  var __SetID: typeof elementPapi.__SetID;
  var __GetID: typeof elementPapi.__GetID;
  var __GetTag: typeof elementPapi.__GetTag;
  var __GetChildren: typeof elementPapi.__GetChildren;
  var __GetAttributeByName: typeof elementPapi.__GetAttributeByName;
  var __GetAttributeNames: typeof elementPapi.__GetAttributeNames;
  var __GetElementUniqueID: typeof elementPapi.__GetElementUniqueID;
  var __SetInlineStyles: typeof elementPapi.__SetInlineStyles;
  var __SetCSSId: typeof elementPapi.__SetCSSId;
  var __SetAttribute: typeof elementPapi.__SetAttribute;
  var __UpdateListCallbacks: typeof elementPapi.__UpdateListCallbacks;
  var __AddEvent: typeof elementPapi.__AddEvent;
  var __GetEvent: typeof elementPapi.__GetEvent;
  var __GetEvents: typeof elementPapi.__GetEvents;
  var __SetEvents: typeof elementPapi.__SetEvents;
  var __AddEventListener: typeof elementPapi.__AddEventListener;
  var __RemoveEventListener: typeof elementPapi.__RemoveEventListener;
  var __StopPropagation: typeof elementPapi.__StopPropagation;
  var __StopImmediatePropagation: typeof elementPapi.__StopImmediatePropagation;
  var __FlushElementTree: typeof elementPapi.__FlushElementTree;
}
