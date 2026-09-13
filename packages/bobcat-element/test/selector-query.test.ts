import { describe, expect, it, rstest } from '@rstest/core';
import { SelectorQuery } from '../src/selector-query.ts';

describe('native SelectorQuery task semantics', () => {
  it('branches without changing the original queue, and exec replays each branch', () => {
    const send = rstest.fn(), report = rstest.fn();
    const original = new SelectorQuery(send, report);
    const first = original.select('#a').setNativeProps({text:'a'});
    const second = first?.select('#b').fields({id:true});
    original.exec();
    expect(send).not.toHaveBeenCalled();
    second?.exec();
    first?.exec();
    expect(send.mock.calls.map(call => [call[0], call[1].identifier])).toEqual([
      ['setNativeProps','#a'], ['fields','#b'], ['setNativeProps','#a'],
    ]);
  });

  it('captures selection roots and constructs query objects from native unique IDs', () => {
    const send = rstest.fn(), callback = rstest.fn();
    const query = new SelectorQuery(send, rstest.fn(), 'component').setRoot(42);
    const nodes = query.select('.item');
    query.setRoot(99);
    nodes.fields({id:true, query:true}, callback)?.exec();
    const [operation, token, fields, complete] = send.mock.calls[0] ?? [];
    expect(operation).toBe('fields');
    expect(token).toEqual({type:0, identifier:'.item', component_id:'component', first_only:true, root_unique_id:42});
    expect(fields).toEqual(['id','unique_id']);
    const status = {code:0, data:'success'};
    complete({data:{id:'chosen', unique_id:51}, status});
    const result = callback.mock.calls[0]?.[0];
    expect(result.id).toBe('chosen');
    expect(Object.hasOwn(result, 'unique_id')).toBe(false);
    result.query.selectAll('view').fields({tag:true})?.exec();
    expect(send.mock.calls[1]?.[1]).toEqual({type:0, identifier:'view', component_id:'', first_only:false, root_unique_id:51});
    expect(callback.mock.calls[0]?.[1]).toBe(status);
    // The native facade uses == true for this option, before its truthy test.
    nodes.fields({query:1})?.exec();
    expect(send.mock.calls[2]?.[2]).toEqual(['unique_id']);
  });

  it('executes legacy ReactRef tasks immediately, and refuses it after queued work', () => {
    const send = rstest.fn(), report = rstest.fn();
    const query = new SelectorQuery(send, report);
    expect(query.selectReactRef('ref')?.setNativeProps({text:'now'})).toBeUndefined();
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0]?.[1].type).toBe(1);
    const queued = new SelectorQuery(send, report).selectRoot().fields({tag:true});
    expect(queued?.selectReactRef('too-late')).toBeUndefined();
    expect(report).toHaveBeenCalledTimes(1);
  });

  it('rejects selectAll invoke locally and preserves success/failure callback payloads', () => {
    const send = rstest.fn(), success = rstest.fn(), fail = rstest.fn();
    const query = new SelectorQuery(send, rstest.fn());
    query.selectAll('.items').invoke({method:'anything', fail})?.exec();
    expect(send).not.toHaveBeenCalled();
    expect(fail).toHaveBeenCalledWith({code:5, data:'selectAll not supported for invoke method'});
    query.select('#target').invoke({method:'test', success, fail})?.exec();
    const complete = send.mock.calls[0]?.[3];
    complete({code:0, data:{width:12}});
    complete({code:2, data:'gone'});
    expect(success).toHaveBeenCalledWith({width:12});
    expect(fail).toHaveBeenLastCalledWith({code:2, data:'gone'});
  });
});
