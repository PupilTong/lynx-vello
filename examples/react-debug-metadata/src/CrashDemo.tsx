import { useState } from '@lynx-js/react';

function innerThrow(): never {
  throw new Error('boom from deep nested call (background)');
}
function mid() {
  innerThrow();
}
function outer() {
  mid();
}

function crashTypeError() {
  const obj = {} as { notAFunction?: () => void };
  return (obj.notAFunction as () => void)();
}

function crashReferenceError(): number {
  // @ts-expect-error intentional undefined reference for the crash demo
  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return notDefinedVariable + 1;
}

function crashExplicit() {
  throw new Error('explicit throw new Error (background)');
}

function crashNested() {
  outer();
}

function crashMainThread() {
  'main thread';
  throw new Error('boom from main-thread');
}

export function CrashDemo() {
  const [, setTick] = useState(0);
  return (
    <view className='crash-section'>
      <text className='crash-title'>CrashDemo (host) — tap a row to throw</text>

      <view className='crash-row' bindtap={() => crashTypeError()}>
        <text>1. TypeError (call undefined, background)</text>
      </view>
      <view className='crash-row' bindtap={() => crashReferenceError()}>
        <text>2. ReferenceError (background)</text>
      </view>
      <view className='crash-row' bindtap={() => crashExplicit()}>
        <text>3. throw new Error (background)</text>
      </view>
      <view className='crash-row' bindtap={() => crashNested()}>
        <text>4. nested deep stack (background)</text>
      </view>
      <view className='crash-row' main-thread:bindtap={crashMainThread}>
        <text>7. main-thread error</text>
      </view>

      <view className='crash-row' bindtap={() => setTick((n) => n + 1)}>
        <text>(tap me: no-op)</text>
      </view>
    </view>
  );
}
