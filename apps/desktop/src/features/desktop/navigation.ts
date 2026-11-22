export const pages = [
  { id: 'home', title: '首页', subtitle: '本机能力与连接概览', icon: 'home' },
  {
    id: 'devices',
    title: '设备',
    subtitle: '管理你已授权的电脑',
    icon: 'devices',
  },
  {
    id: 'connect',
    title: '连接',
    subtitle: '与另一台电脑建立经过同意的连接',
    icon: 'link',
  },
  {
    id: 'sessions',
    title: '会话',
    subtitle: '查看真实会话与连接记录',
    icon: 'sessions',
  },
  {
    id: 'settings',
    title: '设置',
    subtitle: '本机信息与安全状态',
    icon: 'settings',
  },
] as const;
export type PageId = (typeof pages)[number]['id'];
export function pageFromHash(hash: string): PageId {
  return pages.find((p) => hash === `#/${p.id}`)?.id ?? 'home';
}
export function pageLink(page: PageId): string {
  return `#/${page}`;
}
