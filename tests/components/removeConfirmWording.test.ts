import { describe, expect, it } from "vitest";
import { removeConfirmMessageKey } from "@/components/relay/removeConfirmWording";

/**
 * 闸：**删除确认框不能对没登录过的行说「会删掉登录态」。**
 *
 * ## 它守的是什么缺陷（用户实测指出）
 *
 * 那句文案原来是无条件的一句：「会删掉「X」的**登录态**，连带它已生成的 N 个档位。
 * 已充值的余额留在中转站那边，重新登录就能再用。」—— **它不看那一行有没有登录态**。
 *
 * 于是用户删一个只探测过、从没登录的占位行时，被告知要删掉一个不存在的登录态、
 * 以及一笔他从没充过的余额。三句话里有两句不属实。
 *
 * ## 为什么由后端状态决定
 *
 * `loggedIn` 只说「此刻凭据还能用」，不是「这行是否曾经登录过」。凭据过期的行
 * 仍可能有 token 与 account_id，删它确实会删掉登录态。这个事实由后端转换成
 * `status`，前端只选择对应文案，避免各处重复组合布尔值。
 *
 * 具体语义见后端 DTO 的 `RelayRowStatus`；`sessionExpired` 只是兼容已有展示字段，
 * 不是删除文案的判据。
 */
describe("删除确认框的文案分支", () => {
  it("从没登录过的行：不提登录态，也不提余额", () => {
    expect(removeConfirmMessageKey({ status: "notLoggedIn" })).toBe(
      "loongport.row.removeConfirmMessageNeverLoggedIn",
    );
  });

  it("登录着的行：说清会删掉登录态", () => {
    expect(removeConfirmMessageKey({ status: "ready" })).toBe(
      "loongport.row.removeConfirmMessage",
    );
  });

  /**
   * 这条是**这个判据存在的理由**：凭据过期的行库里仍有 token 与 account_id，
   * 删它真的会删掉登录态。判据退化成 `!loggedIn` 时这条会红。
   */
  it("登录过但凭据已过期：仍然会删掉登录态，走同一句", () => {
    expect(removeConfirmMessageKey({ status: "sessionExpired" })).toBe(
      "loongport.row.removeConfirmMessage",
    );
  });
});
