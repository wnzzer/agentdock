import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createSessionStream, SESSION_STREAM_TIMEOUT_MS, SESSION_STREAM_TIMEOUT, SESSION_STREAM_ERROR, SESSION_STREAM_CLOSED, SESSION_STREAM_RENDER_ERROR } from './session-stream.ts';

class FakeSocket {
  readyState=0;binaryType='blob';onopen=null;onmessage=null;onerror=null;onclose=null;sent=[];closes=0;throwSend=false;
  open(){this.readyState=1;this.onopen?.({});}
  message(data){this.onmessage?.({data});}
  error(){this.onerror?.({});}
  remoteClose(){this.readyState=3;this.onclose?.({});}
  close(){this.closes++;this.remoteClose();}
  send(data){if(this.throwSend)throw Error('send failed');this.sent.push(data);}
}
function fakeClock(){
  let now=0,id=0;const pending=new Map();
  return {
    pending,
    setTimer(callback,delay){const key=++id;pending.set(key,{callback,at:now+delay});return key;},
    clearTimer(key){pending.delete(key);},
    advance(ms){now+=ms;for(const [key,timer] of [...pending].sort((a,b)=>a[1].at-b[1].at)){if(timer.at<=now&&pending.delete(key))timer.callback();}},
  };
}
function harness(extra={}){
  const clock=fakeClock(),sockets=[],states=[],messages=[];let opens=0;
  const stream=createSessionStream({createSocket(){const socket=new FakeSocket();sockets.push(socket);return socket;},onState(state,notice){states.push({state,notice});},onMessage(data){messages.push(data);},onOpen(){opens++;},setTimer:clock.setTimer,clearTimer:clock.clearTimer,...extra});
  return{stream,clock,sockets,states,messages,get opens(){return opens;}};
}

test('a stalled handshake leaves connecting after eight seconds and never retries automatically',()=>{
  const h=harness();h.stream.connect('ws://fixture/session');
  assert.equal(h.states.at(-1).state,'connecting');assert.equal(h.clock.pending.size,1);
  h.clock.advance(SESSION_STREAM_TIMEOUT_MS-1);assert.equal(h.states.at(-1).state,'connecting');
  h.clock.advance(1);assert.deepEqual(h.states.at(-1),{state:'error',notice:SESSION_STREAM_TIMEOUT});
  assert.equal(h.sockets[0].closes,1);assert.equal(h.clock.pending.size,0);
  h.clock.advance(60_000);assert.equal(h.sockets.length,1);
});

test('opening cancels the deadline and forwards stream data with a connected state',()=>{
  const h=harness();h.stream.connect('ws://fixture/session');
  const timer=[...h.clock.pending.values()][0].callback;
  h.sockets[0].open();assert.equal(h.clock.pending.size,0);assert.equal(h.opens,1);
  assert.equal(h.sockets[0].binaryType,'arraybuffer');
  h.sockets[0].message('output');assert.deepEqual(h.messages,['output']);
  timer();h.clock.advance(60_000);assert.equal(h.states.at(-1).state,'connected');
});

test('WebSocket error is visible, cancels handshake and cannot be overwritten by a trailing close',()=>{
  const h=harness();h.stream.connect('ws://fixture/session');const socket=h.sockets[0],oldClose=socket.onclose;
  socket.error();oldClose({});
  assert.deepEqual(h.states.at(-1),{state:'error',notice:SESSION_STREAM_ERROR});
  assert.equal(h.clock.pending.size,0);assert.equal(socket.onopen,null);assert.equal(socket.onmessage,null);assert.equal(socket.onerror,null);assert.equal(socket.onclose,null);
});

test('remote close before or after handshake becomes disconnected with a recoverable notice',()=>{
  for(const opened of [false,true]){
    const h=harness();h.stream.connect('ws://fixture/session');if(opened)h.sockets[0].open();h.sockets[0].remoteClose();
    assert.deepEqual(h.states.at(-1),{state:'disconnected',notice:SESSION_STREAM_CLOSED});assert.equal(h.clock.pending.size,0);
  }
});

test('reconnect retires every old handler and stale open/message/error/close/deadline cannot pollute the replacement',()=>{
  const h=harness();h.stream.connect('ws://fixture/first');const old=h.sockets[0];
  const stale={open:old.onopen,message:old.onmessage,error:old.onerror,close:old.onclose,timer:[...h.clock.pending.values()][0].callback};
  h.stream.connect('ws://fixture/second');h.sockets[1].open();const count=h.states.length;
  stale.open({});stale.message({data:'old output'});stale.error({});stale.close({});stale.timer();
  assert.equal(h.states.length,count);assert.deepEqual(h.messages,[]);assert.equal(h.states.at(-1).state,'connected');
  assert.equal(old.onopen,null);assert.equal(old.closes,1);assert.equal(h.clock.pending.size,0);
});

test('inputs are sent only on the current open socket and are never queued or replayed',()=>{
  const h=harness();h.stream.connect('ws://fixture/first');
  assert.equal(h.stream.send('before-open'),false);h.sockets[0].open();assert.equal(h.stream.send('one-input'),true);
  h.stream.connect('ws://fixture/second');assert.equal(h.stream.send('between-sockets'),false);h.sockets[1].open();
  assert.deepEqual(h.sockets[0].sent,['one-input']);assert.deepEqual(h.sockets[1].sent,[]);
  h.sockets[1].throwSend=true;assert.equal(h.stream.send('failed-input'),false);assert.equal(h.states.at(-1).state,'error');
  h.stream.connect('ws://fixture/third');h.sockets[2].open();assert.deepEqual(h.sockets[2].sent,[]);
});

test('socket construction and renderer exceptions produce visible bounded errors',()=>{
  const failed=harness({createSocket(){throw Error('constructor failed with private URL');}});
  assert.doesNotThrow(()=>failed.stream.connect('ws://fixture'));
  assert.deepEqual(failed.states.at(-1),{state:'error',notice:SESSION_STREAM_ERROR});assert.equal(failed.clock.pending.size,0);
  const render=harness({onMessage(){throw Error('renderer unavailable');}});render.stream.connect('ws://fixture');render.sockets[0].open();render.sockets[0].message(new ArrayBuffer(2));
  assert.deepEqual(render.states.at(-1),{state:'error',notice:SESSION_STREAM_RENDER_ERROR});assert.equal(render.sockets[0].closes,1);
});

test('explicit disconnect and disposal cancel deadlines and invalidate all outstanding events',()=>{
  const h=harness();h.stream.connect('ws://fixture');const staleOpen=h.sockets[0].onopen;
  h.stream.disconnect();assert.equal(h.clock.pending.size,0);staleOpen({});assert.equal(h.states.at(-1).state,'disconnected');
  h.stream.connect('ws://fixture/new');const staleError=h.sockets[1].onerror;h.stream.dispose();const count=h.states.length;
  staleError({});h.clock.advance(60_000);h.stream.connect('ws://fixture/ignored');
  assert.equal(h.states.length,count);assert.equal(h.sockets.length,2);assert.equal(h.clock.pending.size,0);assert.equal(h.stream.send('ignored'),false);
});

test('a native exit retains exited state even if a close event was already queued',()=>{
  const h=harness();h.stream.connect('ws://fixture');h.sockets[0].open();const staleClose=h.sockets[0].onclose;
  h.stream.finish('Process exited.');staleClose({});
  assert.deepEqual(h.states.at(-1),{state:'exited',notice:'Process exited.'});assert.equal(h.stream.send('after-exit'),false);
});
