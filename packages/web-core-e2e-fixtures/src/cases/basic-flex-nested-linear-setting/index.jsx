/*
// Copyright 2024 The Lynx Authors. All rights reserved.
// Licensed under the Apache License Version 2.0 that can be found in the
// LICENSE file in the root directory of this source tree.
*/
import { Component, root } from '@lynx-js/react';
class Page extends Component {
  render() {
    return (
      <view
        id='parent'
        style={{
          height: '300px',
          backgroundColor: 'yellow',
          display: 'flex',
          justifyContent: 'space-between',
          flexDirection: 'column',
          flexWrap: 'wrap',
        }}
      >
        <view
          id='child'
          style={{
            display: 'linear',
            linearGravity: 'space-between',
            height: '100px',
            width: '100px',
            backgroundColor: 'red',
          }}
        >
          <view style={{ height: '20px', width: '20px', backgroundColor: 'blue' }} />
          <view style={{ height: '20px', width: '20px', backgroundColor: 'blue' }} />
        </view>
      </view>
    );
  }
}
root.render(<Page></Page>);
