package main

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/blanklabel/meldworld/model"
	"github.com/gorilla/websocket"
)

func showMap(gamemap model.WorldMap) {
	fmt.Println("showing map")
	b := make([][]string, gamemap.Map.Width)
	for i := range b {
		b[i] = make([]string, gamemap.Map.Height)
		for v := range b[i] {
			b[i][v] = "X"
		}
	}

	for _, something := range gamemap.Entities {
		b[something.Coordinates.Y][something.Coordinates.X] = "@"
	}

	for i := range b {
		fmt.Println(b[i])
	}
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

	r := &model.ClientMessage{ModelType: model.ModelType{MsgType: model.CLIENTMESSAGE},
		Msg: "move and groove"}
	wsConn.WriteJSON(r)

	cmsg := &model.ModelType{}
	m := &model.WorldMap{}

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
		case model.CLIENTMESSAGE:
			m := &model.ClientMessage{}
			json.Unmarshal(jsonData, m)
			fmt.Println("Recieved:", m.Msg, " From: ", m.Sender)

		// receive bootstap of map
		case model.WORLDMAP:
			json.Unmarshal(jsonData, m)
			showMap(*m)
		}
	}

}
