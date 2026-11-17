/// <reference types="vite/client" />

/** 由 vite.config.ts 在编译期注入（取自 app/package.json 的 version）。
 *  「关于」面板读它，避免再把版本号写死在界面里。 */
declare const __APP_VERSION__: string;
