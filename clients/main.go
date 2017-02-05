package main

import (
	"fmt"
	"net/http"

	"github.com/blanklabel/meldworld/entity"
	"github.com/blanklabel/meldworld/mapper"
	"github.com/gorilla/websocket"
)

type ClientMessage struct {
	MsgType string `json:"type"`
	Msg     string `json:"msg"`
	Sender  string `json:"sender"`
}

type WorldMap struct {
	Type string
	mapper.MapObj
	entity.EntityObj
}

func main() {
	d := websocket.Dialer{}

	wsHeaders := http.Header{
		"Origin": {"http://localhost:8080"},
		"Sec-WebSocket-Extensions": {"permessage-deflate; client_max_window_bits," +
			" x-webkit-deflate-frame"},
	}

	wsConn, resp, err := d.Dial("ws://localhost:8080/game", wsHeaders)
	fmt.Println(wsConn, resp, err)
	b := []byte("move and groove")
	wsConn.WriteMessage(1, b)
	cmsg := &WorldMap{}
	for {
		wsConn.ReadJSON(cmsg)
		fmt.Println(cmsg)
	}

}
