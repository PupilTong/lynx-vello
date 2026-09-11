// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';

type BundleStats = {
  toJson: (options: typeof BUNDLE_STATS_JSON_OPTIONS) => BundleStatsJson;
};
type AfterBuildResult = { stats?: BundleStats };
type BundleStatsPluginAPI = {
  context: { distPath: string };
  onAfterBuild: (callback: (result: AfterBuildResult) => void) => void;
};
type BundleStatsPlugin = {
  name: string;
  setup: (api: BundleStatsPluginAPI) => void;
};
type BundleStatsJson = {
  name?: string;
  children?: BundleStatsJson[];
  [key: string]: unknown;
};

export const BUNDLE_STATS_JSON_OPTIONS = {
  assets: true,
  chunks: true,
  modules: true,
  entrypoints: true,
  chunkGroups: true,
};

export function pluginLynxBundleAnalysisStats(): BundleStatsPlugin {
  return {
    name: 'example:lynx-bundle-analysis-stats',
    setup(api: BundleStatsPluginAPI) {
      if (!process.env['RSPEEDY_BUNDLE_ANALYSIS']) {
        return;
      }

      const writeLynxStatsJson = ({ stats }: AfterBuildResult) => {
        if (!stats) {
          return;
        }

        const statsPath = path.join(api.context.distPath, 'stats.json');
        mkdirSync(path.dirname(statsPath), { recursive: true });
        writeFileSync(
          statsPath,
          JSON.stringify(
            getLynxBundleStatsJson(stats.toJson(BUNDLE_STATS_JSON_OPTIONS)),
            null,
            2,
          ),
        );
      };

      api.onAfterBuild(writeLynxStatsJson);
    },
  };
}

export function getLynxBundleStatsJson(
  statsJson: BundleStatsJson,
): BundleStatsJson {
  if (!statsJson.children || statsJson.children.length === 0) {
    return withoutEmptyChildren(statsJson);
  }

  const lynxStatsJson = statsJson.children.find(child =>
    isLynxStatsChild(child.name)
  );
  const fallbackStatsJson = statsJson.children[0];
  if (!fallbackStatsJson) {
    return withoutEmptyChildren(statsJson);
  }

  return withoutEmptyChildren(lynxStatsJson ?? fallbackStatsJson);
}

function isLynxStatsChild(name: string | undefined): boolean {
  return name === 'lynx' || name?.startsWith('lynx-') === true;
}

function withoutEmptyChildren(statsJson: BundleStatsJson): BundleStatsJson {
  if (!statsJson.children || statsJson.children.length > 0) {
    return statsJson;
  }

  const statsJsonWithoutEmptyChildren = { ...statsJson };
  delete statsJsonWithoutEmptyChildren.children;
  return statsJsonWithoutEmptyChildren;
}
