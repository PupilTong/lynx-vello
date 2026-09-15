// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.

import { useState } from '@lynx-js/react';

import './index.scss';

import ExplorerIconDark from '@assets/images/explorer-dark.png?inline';
import ExplorerIcon from '@assets/images/explorer.png?inline';
import ScanIconDark from '@assets/images/scan-dark.png?inline';
import ScanIcon from '@assets/images/scan.png?inline';
import ShowcaseIcon from '@assets/images/showcase.png?inline';
import type { BaseEvent, InputInputEvent } from '@lynx-js/types';
import {
  openSchema,
  navigateTo,
  useTheme,
  useSafeArea,
  getRecentSessions,
  clearRecentSessions,
  parseLaunchCommand,
  setCommandBoolean,
  setCommandTheme,
} from '@explorer/lib';
import type { LaunchSession } from '@explorer/lib';
import ListRow from '@components/list-row';

interface HomePageProps {
  showPage: boolean;
}

export default function HomePage(props: HomePageProps) {
  const { resolved, withTheme } = useTheme();
  const safeArea = useSafeArea();
  const [inputValue, setInputValue] = useState('');
  const [recentSessions, setRecentSessions] = useState<LaunchSession[]>(
    getRecentSessions()
  );
  const command = parseLaunchCommand(inputValue);

  const icons = {
    Scan: { dark: ScanIconDark, light: ScanIcon },
    Explorer: { dark: ExplorerIconDark, light: ExplorerIcon },
  } as const;

  const openScan = () => {
    'background only';
    NativeModules.ExplorerModule.openScan();
  };

  const refreshRecentAfterOpen = (pending: Promise<void>) => {
    'background only';
    void pending
      .then(() => {
        setRecentSessions(getRecentSessions());
      })
      .catch(() => {
        // Navigation reports the native error before rejecting.
      });
  };

  const handleOpen = () => {
    'background only';
    if (inputValue && inputValue.length > 0) {
      refreshRecentAfterOpen(openSchema(inputValue));
    }
  };

  const openShowcasePage = () => {
    'background only';
    void navigateTo('showcase/menu/main.lynx.bundle', {
      title: 'Showcase',
      title_color: resolved === 'dark' ? 'FFFFFF' : '000000',
      bar_color: resolved === 'dark' ? '181D25' : 'F0F2F5',
      back_button_style: resolved,
    }).catch(() => {
      // Navigation reports the native error before rejecting.
    });
  };

  const handleInput = (event: BaseEvent<'bindinput', InputInputEvent>) => {
    'background only';
    const currentValue = event.detail.value.trim();
    setInputValue(currentValue);
  };

  const handleThemeToggle = (theme: 'dark' | 'light') => {
    'background only';
    setInputValue(
      setCommandTheme(command, command.theme === theme ? null : theme)
    );
  };

  const handleThemeDefault = () => {
    'background only';
    setInputValue(setCommandTheme(command, null));
  };

  const handleFullscreenToggle = () => {
    'background only';
    setInputValue(
      setCommandBoolean(command, 'fullscreen', !command.fullscreen)
    );
  };

  const handleClearRecent = () => {
    'background only';
    clearRecentSessions();
    setRecentSessions([]);
  };

  const handleOpenRecent = (session: LaunchSession) => {
    'background only';
    refreshRecentAfterOpen(openSchema(session.url, session.source));
  };

  const sessionSummary = (session: LaunchSession) => {
    const details = [
      session.fullscreen ? 'Fullscreen' : null,
      session.hiddenNav && !session.fullscreen ? 'No navigation' : null,
      session.theme
        ? `${session.theme.charAt(0).toUpperCase()}${session.theme.slice(1)}`
        : null,
    ].filter(Boolean);
    return details.length ? details.join(' · ') : 'Default presentation';
  };

  const getIcon = (name: keyof typeof icons) => icons[name][resolved];
  const getTextColor = () => (resolved === 'dark' ? '#FFFFFF' : '#000000');

  if (!props.showPage) {
    return <></>;
  }

  const navigatorHeight = 48 + safeArea.bottom;
  const horizontalSafeArea = safeArea.left + safeArea.right;
  const screenWidth = Number(lynx.__globalProps.screenWidth || 0);
  const sessionURLLineCount = (url: string) => {
    const contentWidth = Math.max(180, screenWidth * 0.9 - 64);
    const estimatedCharactersPerLine = Math.max(
      24,
      Math.floor(contentWidth / 7.2)
    );
    return Math.max(1, Math.ceil(url.length / estimatedCharactersPerLine));
  };
  const sessionRowHeight = (url: string) => 40 + sessionURLLineCount(url) * 18;
  const isDesktop =
    lynx.__globalProps.platform === 'macos' ||
    lynx.__globalProps.platform === 'windows';

  return (
    <scroll-view
      scroll-y
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
          <image src={getIcon('Explorer')} className="logo" mode="aspectFit" />
          <text className={withTheme('home-title')}>Lynx Explorer</text>
          <view className="scan">
            <image
              src={getIcon('Scan')}
              className="scan-icon"
              bindtap={openScan}
              accessibility-element={true}
              accessibility-label="Open Scan"
              accessibility-traits="button"
            />
          </view>
        </view>

        <view
          className={withTheme('input-card-url')}
          style={{
            minHeight: isDesktop ? '40%' : '142px',
          }}
        >
          <view className="bundle-card-header">
            <view className="bundle-card-icon" />
            <view className="bundle-card-heading">
              <text className={withTheme('bold-text')}>Bundle URL</text>
            </view>
          </view>
          <view className="bundle-input-row">
            <input
              className={withTheme('input-box')}
              value={inputValue}
              bindinput={handleInput}
              placeholder="Enter Bundle URL"
              text-color={getTextColor()}
              lynx-test-tag="bundle-url-input"
              ios-platform-accessibility-id="bundle-url-input"
              accessibility-element={true}
              accessibility-label="Bundle URL input"
            />
            <view
              className={withTheme('open-button')}
              bindtap={handleOpen}
              lynx-test-tag="open-bundle-url"
              ios-platform-accessibility-id="open-bundle-url"
              accessibility-element={true}
              accessibility-label="Open bundle URL"
              accessibility-traits="button"
            >
              <text className="open-button-text" accessibility-element={false}>
                Open
              </text>
            </view>
          </view>
          <view className="launch-options">
            <view
              className={
                command.fullscreen
                  ? 'launch-chip launch-chip--active'
                  : withTheme('launch-chip')
              }
              bindtap={handleFullscreenToggle}
              accessibility-element={true}
              accessibility-label="Toggle fullscreen"
              accessibility-traits="button"
            >
              <text
                className={
                  command.fullscreen
                    ? 'launch-chip-text launch-chip-text--active'
                    : withTheme('launch-chip-text')
                }
              >
                Fullscreen
              </text>
            </view>
            <view
              className={
                command.theme === null
                  ? 'launch-chip launch-chip--active'
                  : withTheme('launch-chip')
              }
              bindtap={handleThemeDefault}
              accessibility-element={true}
              accessibility-label="Use default page chrome"
              accessibility-traits="button"
            >
              <text
                className={
                  command.theme === null
                    ? 'launch-chip-text launch-chip-text--active'
                    : withTheme('launch-chip-text')
                }
              >
                Default
              </text>
            </view>
            {(['dark', 'light'] as const).map((theme) => (
              <view
                key={theme}
                className={
                  command.theme === theme
                    ? 'launch-chip launch-chip--active'
                    : withTheme('launch-chip')
                }
                bindtap={() => handleThemeToggle(theme)}
                accessibility-element={true}
                accessibility-label={`Use ${theme} page chrome`}
                accessibility-traits="button"
              >
                <text
                  className={
                    command.theme === theme
                      ? 'launch-chip-text launch-chip-text--active'
                      : withTheme('launch-chip-text')
                  }
                >
                  {theme === 'dark' ? 'Dark' : 'Light'}
                </text>
              </view>
            ))}
          </view>
        </view>

        <view className="showcases-section">
          <view className="showcases-header">
            <text className={withTheme('showcases-title')}>Showcases</text>
          </view>
          <view className={withTheme('showcases-card')}>
            <ListRow
              className="showcases-row"
              onTap={openShowcasePage}
              accessibilityLabel="Open Lynx Showcases"
              leading={<image src={ShowcaseIcon} className="showcase-icon" />}
              title={
                <text className={withTheme('list-row-title')}>
                  Lynx Showcases
                </text>
              }
              showChevron={true}
            />
          </view>
        </view>

        {recentSessions.length > 0 && (
          <view className={withTheme('recent-panel')}>
            <view className="recent-header">
              <text className={withTheme('recent-title')}>Session History</text>
              <text
                className={withTheme('recent-clear')}
                bindtap={handleClearRecent}
                accessibility-element={true}
                accessibility-label="Clear recent history"
                accessibility-traits="button"
              >
                Clear
              </text>
            </view>
            {recentSessions.map((session) => (
              <ListRow
                key={session.id}
                className={withTheme('recent-item')}
                style={{ minHeight: `${sessionRowHeight(session.url)}px` }}
                onTap={() => handleOpenRecent(session)}
                accessibilityLabel={`Open ${session.url}`}
                title={
                  <text
                    className={withTheme('recent-url')}
                    style={{
                      height: `${sessionURLLineCount(session.url) * 18}px`,
                    }}
                  >
                    {session.url}
                  </text>
                }
                subtitle={
                  <view className="session-metadata">
                    <text className={withTheme('session-meta-copy')}>
                      {sessionSummary(session)}
                    </text>
                  </view>
                }
                showChevron={true}
              />
            ))}
          </view>
        )}
      </view>
    </scroll-view>
  );
}
