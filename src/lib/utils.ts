/** 拼类名。shadcn 用 `clsx + tailwind-merge`，这里只做拼接 —— 我们不传冲突的类。 */
export function cn(...values: Array<string | false | null | undefined>): string {
  return values.filter(Boolean).join(" ");
}
