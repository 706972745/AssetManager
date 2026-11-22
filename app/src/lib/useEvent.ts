/**
 * 引用永久稳定的回调（latest-ref 模式）。
 *
 * 为什么需要它：给 `memo` 组件传内联箭头函数，父组件每渲一次都是新引用，
 * 浅比较必然失败 —— `memo` 等于没加。改用 `useCallback` 就要逐个列依赖，
 * 而 App 里这些回调依赖着筛选项 / 选中集 / 列表本身，**漏一项不会报错**，
 * 只会让处理器一直拿着旧闭包跑（点下去没反应，或者用的还是上一批数据）。
 *
 * 这里返回的函数引用不变、调用时才去读 ref 里的最新实现：能过浅比较，
 * 也不会有过期闭包。**只用于事件处理器 / 副作用回调**，别在渲染期间直接调用
 * （渲染期间读到的是上一次渲染的实现，并发渲染下 React 不保证它是最新的）。
 */
import { useCallback, useRef } from "react";

export function useEvent<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
  const ref = useRef(fn);
  ref.current = fn;
  return useCallback((...args: A) => ref.current(...args), []);
}
