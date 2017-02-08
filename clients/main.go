package main

import (
	"fmt"
	"net/http"

	"encoding/json"

	"github.com/blanklabel/meldworld/entity"
	"github.com/blanklabel/meldworld/mapper"
	"github.com/gorilla/websocket"
)

// Simple way to encompass all messages
type ClientMessage struct {
	MsgType string `json:"type"`
	Msg     string `json:"msg"`
	Sender  string `json:"sender"`
}

// Binds map data and entity data
type WorldMap struct {
	Type string
	mapper.MapObj
	entity.EntityObj
}

type Test struct {
	MsgType string `json:"type"`
}

// Simple client
func main() {
	d := websocket.Dialer{}

	wsHeaders := http.Header{
		"Origin": {"http://localhost:8080"},
		"Sec-WebSocket-Extensions": {"permessage-deflate; client_max_window_bits," +
			" x-webkit-deflate-frame"},
	}

	wsConn, resp, err := d.Dial("ws://localhost:8080/game", wsHeaders)
	if err != nil {
		panic("err")
	}
	fmt.Println("Server Response", resp)

	r := &ClientMessage{MsgType: "client.message", Msg: "move and groove"}
	wsConn.WriteJSON(r)

	cmsg := &Test{}
	m := &WorldMap{}

	for {
		_, jsonData, err := wsConn.ReadMessage()

		// umm wat? bye Felica
		if err != nil {
			break
		}

		json.Unmarshal(jsonData, cmsg)

		// Determine message type
		switch cmsg.MsgType {

		// Receive client messages
		case "client.message":
			m := &ClientMessage{}
			json.Unmarshal(jsonData, m)
			fmt.Println("Recieved:", m.Msg, " From: ", m.Sender)

		// receive bootstap of map
		case "worldmap":
			json.Unmarshal(jsonData, m)
			fmt.Println(m.MapObj.Map.Height, m.MapObj.Map.Width)
		}
	}

}
