import { avatars, covers } from './images.ts';

export interface BenchmarkCard {
  id: number;
  coverImage: string;
  title: string;
  authorAvatar: string;
  authorName: string;
  authorDescription: string;
  likes: number;
  liked: boolean;
  commentCount: number;
  shareCount: number;
  tags: string[];
  content: string;
  creationDate: string;
}

// Forty cards retain the original feed workload with neutral Explorer content.
export const posts: { data: BenchmarkCard }[] = Array.from(
  { length: 40 },
  (_, index) => ({
    data: {
      id: index + 1,
      coverImage: covers[index]!,
      title: `Lynx Vello · Rendering sample ${index + 1}`,
      authorAvatar: avatars[index]!,
      authorName: `Explorer ${index + 1}`,
      authorDescription: 'Lynx Vello rendering benchmark',
      likes: 200 + index * 10,
      liked: index % 2 === 0,
      commentCount: 20 + index,
      shareCount: 10 + index,
      tags: ['vello', 'layout', `sample-${index + 1}`],
      content: `Sample ${index + 1} combines text, images and nested views to exercise layout and painting in the Lynx Vello Explorer.`,
      creationDate: '2026-01-01',
    },
  }),
);
