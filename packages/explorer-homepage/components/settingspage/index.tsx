// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

import { useState } from '@lynx-js/react';
import './index.scss';

import AutoDarkIcon from '@assets/images/auto-dark.png?inline';
import AutoLightIcon from '@assets/images/auto.png?inline';
import DarkDarkIcon from '@assets/images/dark-dark.png?inline';
import DarkLightIcon from '@assets/images/dark.png?inline';
import LightDarkIcon from '@assets/images/light-dark.png?inline';
import LightLightIcon from '@assets/images/light.png?inline';
import { navigateTo, useTheme, useSafeArea } from '@explorer/lib';
import type { ThemePreference } from '@explorer/lib';
import ListRow from '@components/list-row';
import BrandIcon from '@components/brand-icon';

const THEMES: ThemePreference[] = ['Auto', 'Light', 'Dark'];

interface SettingsPageProps {
  showPage: boolean;
}

const DESKTOP_DEVTOOL_SWITCH_PARAMS = { width: 420, height: 720 } as const;

export default function SettingsPage(props: SettingsPageProps) {
  const { preference, resolved, setPreference, withTheme } = useTheme();
  const safeArea = useSafeArea();
  const [listAsyncRender, setListAsyncRender] = useState(false);
  const platform = lynx.__globalProps.platform as string | undefined;

  const icons = {
    Auto: { dark: AutoDarkIcon, light: AutoLightIcon },
    Dark: { dark: DarkDarkIcon, light: DarkLightIcon },
    Light: { dark: LightDarkIcon, light: LightLightIcon },
  } as const;

  const openDevtoolSwitchPage = () => {
    const isDesktop = platform === 'macos' || platform === 'windows';
    navigateTo(
      'switchPage/devtoolSwitch.lynx.bundle',
      isDesktop ? DESKTOP_DEVTOOL_SWITCH_PARAMS : undefined
    );
  };

  const selectionControl = (selected: boolean) => (
    <view
      className={
        selected
          ? withTheme('radio-button-container-active')
          : withTheme('radio-button-container-inactive')
      }
    >
      {selected ? <view className={withTheme('radio-button-active')} /> : null}
    </view>
  );

  if (!props.showPage) return <></>;

  const navigatorHeight = 48 + safeArea.bottom;
  const horizontalSafeArea = safeArea.left + safeArea.right;
  const screenWidth = Number(lynx.__globalProps.screenWidth || 0);
  const screenHeight = Number(lynx.__globalProps.screenHeight || 0);
  const landscape = screenWidth > 0 && screenWidth > screenHeight;
  const rowHeight = landscape ? '36px' : '44px';

  const ownershipHeader = (title: string, description: string) => (
    <view className="ownership-header">
      <text className={withTheme('ownership-title')}>{title}</text>
      <text className={withTheme('ownership-description')}>{description}</text>
    </view>
  );

  const appearanceGroup = () => (
    <>
      {ownershipHeader('App', 'Shared by every runtime')}
      <view className={withTheme('settings-group')}>
        <view className="settings-group-label">
          <text className={withTheme('settings-group-label-text')}>
            Appearance
          </text>
        </view>
        {THEMES.map((theme, index) => (
          <ListRow
            key={theme}
            className={`option-item ${index > 0 ? 'settings-row-divider' : ''}`}
            size="compact"
            style={{ height: rowHeight }}
            onTap={() => setPreference(theme)}
            accessibilityLabel={`Set app appearance to ${theme}`}
            leading={
              <image src={icons[theme][resolved]} className="option-icon" />
            }
            title={<text className={withTheme('list-row-title')}>{theme}</text>}
            trailing={selectionControl(preference === theme)}
          />
        ))}
      </view>
    </>
  );

  const developerGroup = () => (
    <>
      {ownershipHeader('Developer', 'Explorer-owned diagnostics and rendering')}
      <view className={withTheme('settings-group')}>
        <ListRow
          className="option-item"
          size="compact"
          style={{ height: rowHeight }}
          onTap={openDevtoolSwitchPage}
          accessibilityLabel="Lynx DevTool Switches"
          title={
            <text className={withTheme('list-row-title')}>
              DevTool Switches
            </text>
          }
          showChevron={true}
        />
        <ListRow
          className="option-item settings-row-divider"
          size="compact"
          style={{ height: rowHeight }}
          onTap={() => {
            NativeModules.ExplorerModule.setThreadMode(
              !listAsyncRender ? 1 : 0
            );
            setListAsyncRender(!listAsyncRender);
          }}
          accessibilityLabel="List Async Render"
          title={
            <text className={withTheme('list-row-title')}>
              List Async Render
            </text>
          }
          trailing={selectionControl(listAsyncRender)}
        />
      </view>
    </>
  );

  const runtimeGroup = () => (
    <>
      {ownershipHeader('Runtime', 'Installed engines and extensions')}
      <view className={withTheme('settings-group')}>
        <ListRow
          className="option-item"
          size="compact"
          style={{ height: rowHeight }}
          accessibilityLabel={`Lynx Engine ${SystemInfo.engineVersion}`}
          leading={
            <BrandIcon
              color={resolved === 'dark' ? '#FFFFFF' : '#111113'}
              className="runtime-icon runtime-icon--lynx"
            />
          }
          title={
            <text className={withTheme('list-row-title')}>Lynx Engine</text>
          }
          trailing={
            <text className={withTheme('info-value')}>
              {SystemInfo.engineVersion}
            </text>
          }
        />
      </view>
    </>
  );

  return (
    <scroll-view
      scroll-y
      clip-radius="true"
      className={withTheme('page')}
      style={{ height: `calc(100% - ${navigatorHeight}px)` }}
    >
      <view
        className="safe-area-content"
        style={{
          marginLeft: `${safeArea.left}px`,
          marginRight: `${safeArea.right}px`,
          width: `calc(100% - ${horizontalSafeArea}px)`,
        }}
      >
        <view
          className="page-header"
          style={{ marginTop: `${Math.max(safeArea.top, 10)}px` }}
        >
          <text className={withTheme('title')}>Settings</text>
        </view>

        {landscape ? (
          <view className="settings-columns">
            <view className="settings-column">{appearanceGroup()}</view>
            <view className="settings-column">
              {developerGroup()}
              {runtimeGroup()}
            </view>
          </view>
        ) : (
          <>
            {appearanceGroup()}
            {developerGroup()}
            {runtimeGroup()}
          </>
        )}
        <view style={{ height: `${navigatorHeight}px` }} />
      </view>
    </scroll-view>
  );
}
