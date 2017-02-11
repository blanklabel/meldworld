package main

import (
	"fmt"
	"net/http"

	"encoding/json"

	"github.com/blanklabel/meldworld/model"
	"github.com/gorilla/websocket"
)

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

	r := &model.ClientMessage{MsgType: "client.message", Msg: "move and groove"}
	wsConn.WriteJSON(r)

	cmsg := &model.ModelType{}
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
			m := &model.ClientMessage{}
			json.Unmarshal(jsonData, m)
			fmt.Println("Recieved:", m.Msg, " From: ", m.Sender)

		// receive bootstap of map
		case "worldmap":
			json.Unmarshal(jsonData, m)
			fmt.Println(m.MapObj.Map.Height, m.MapObj.Map.Width)
		}
	}

}
