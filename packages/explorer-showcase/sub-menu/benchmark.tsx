import { root } from '@lynx-js/react';
import { AppContextProvider } from '@explorer/lib';
import { Menu } from '@components/menu';
import { benchmarkCases } from '../benchmark/catalog.ts';

const items = benchmarkCases.map(({ id, title, description }) => ({
  title,
  description,
  url: `file://lynx?local://showcase/menu/benchmark/${id}.lynx.bundle`,
}));

root.render(
  <AppContextProvider>
    <Menu items={items} />
  </AppContextProvider>,
);
