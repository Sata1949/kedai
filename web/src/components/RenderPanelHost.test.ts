// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import RenderPanelHost from './RenderPanelHost.vue';

describe('RenderPanelHost 画布宿主', () => {
  it('默认保持原画布底色;transparent 时只让出 background-color', async () => {
    const opaque = mount(RenderPanelHost);
    expect(opaque.find('.sv-canvas').classes()).not.toContain('sv-canvas--ambient');

    const transparent = mount(RenderPanelHost, { props: { transparent: true } });
    expect(transparent.find('.sv-canvas').classes()).toContain('sv-canvas--ambient');
    // hydrate 在空画布上是幂等 no-op,不得抛错
    await expect(transparent.vm.hydrate()).resolves.toBeUndefined();
    opaque.unmount();
    transparent.unmount();
  });
});
