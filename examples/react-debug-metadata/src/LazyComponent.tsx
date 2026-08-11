import './LazyComponent.css';

function lazyDeepInner(): never {
  throw new Error('boom from deep nested call (LazyComponent, background)');
}
function lazyDeepMid() {
  lazyDeepInner();
}

function lazyCrashBackground() {
  lazyDeepMid();
}

function lazyCrashType() {
  const obj = {} as { gone?: () => void };
  return (obj.gone as () => void)();
}

function lazyCrashUndefinedProp() {
  const obj = {} as { missing?: { x: number } };
  return obj.missing!.x;
}

function lazyCrashMainThread() {
  'main thread';
  throw new Error('boom from LazyComponent main-thread');
}

export default function LazyComponent() {
  return (
    <view className='crash-section'>
      <text className='LazyComponent'>
        LazyComponent (dynamic) — tap to throw
      </text>
      <view className='crash-row' bindtap={() => lazyCrashBackground()}>
        <text>L1. nested deep stack (dynamic, background)</text>
      </view>
      <view className='crash-row' bindtap={() => lazyCrashType()}>
        <text>L2. TypeError (dynamic, background)</text>
      </view>
      <view className='crash-row' main-thread:bindtap={lazyCrashMainThread}>
        <text>L3. main-thread error (dynamic)</text>
      </view>
      <view className='crash-row' bindtap={() => lazyCrashUndefinedProp()}>
        <text>L4. read .x of undefined (dynamic, background)</text>
      </view>
    </view>
  );
}
