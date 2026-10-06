import {root} from '@lynx-js/react';
import './index.css';

// Six 120px `<x-swiper>`s of a red, a green and a blue item, in a 240px by
// 360px grid at the page origin, styled by the engine's UA sheet beyond their
// sizes and colours. Each shows its three dots along its bottom (along its
// right edge when vertical), the current item's in white, unless it says
// otherwise:
//
// - top left: `current={2}`, so it starts on the blue item;
// - top right: `vertical`, `current={1}`, so it starts on the green item;
// - bottom left: `mode='coverflow'`, `current={1}`: the green item centred at
//   full size, the red one scaled down at its left;
// - bottom right: `autoplay` every second, so it turns from red to green to
//   blue and stays there; `smooth-scroll`, present, makes each turn instant
//   (web-core's reading of the attribute), which an offscreen painter shows
//   at once where a smooth glide would wait for display frames;
// - third row left: `indicator-dots={false}`, so no dots;
// - third row right: yellow dots with a cyan current one.
function App() {
  return (
    <view class='grid'>
      <x-swiper class='swiper top left' current={2}>
        <x-swiper-item class='red' />
        <x-swiper-item class='green' />
        <x-swiper-item class='blue' />
      </x-swiper>
      <x-swiper class='swiper top right' vertical current={1}>
        <x-swiper-item class='red' />
        <x-swiper-item class='green' />
        <x-swiper-item class='blue' />
      </x-swiper>
      <x-swiper class='swiper bottom left' mode='coverflow' current={1}>
        <x-swiper-item class='red' />
        <x-swiper-item class='green' />
        <x-swiper-item class='blue' />
      </x-swiper>
      <x-swiper class='swiper bottom right' autoplay interval={1000} smooth-scroll>
        <x-swiper-item class='red' />
        <x-swiper-item class='green' />
        <x-swiper-item class='blue' />
      </x-swiper>
      <x-swiper class='swiper third left' indicator-dots={false}>
        <x-swiper-item class='red' />
        <x-swiper-item class='green' />
        <x-swiper-item class='blue' />
      </x-swiper>
      <x-swiper
        class='swiper third right'
        indicator-color='rgb(255, 255, 0)'
        indicator-active-color='rgb(0, 255, 255)'
      >
        <x-swiper-item class='red' />
        <x-swiper-item class='green' />
        <x-swiper-item class='blue' />
      </x-swiper>
    </view>
  );
}

root.render(<App />);
