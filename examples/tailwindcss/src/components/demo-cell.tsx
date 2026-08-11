import type { ReactNode } from '@lynx-js/react';

interface DemoCellProps {
  label: string;
  className?: string;
  /** Content rendered inside the sample block. Defaults to "Sample". */
  children?: ReactNode;
}

/** A labeled visual sample for Tailwind classes. */
export function DemoCell(
  { label, className = '', children }: DemoCellProps,
) {
  return (
    <view className='w-full flex flex-col bg-paper-clear p-[16px] rounded-[12px]'>
      <text className='text-sm text-content-muted'>
        {label}
      </text>
      <view
        className={`w-full mt-[12px] rounded-lg flex flex-col ${className}`}
      >
        {children ?? <text className='text-content'>Sample</text>}
      </view>
    </view>
  );
}
