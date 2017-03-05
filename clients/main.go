package main

import (
	"encoding/json"
	"fmt"
	"net/http"

	"os"
	"os/exec"
	"runtime"
	"time"

	"github.com/blanklabel/meldworld/model"
	"github.com/gorilla/websocket"
)

func ClearScreen() {
	switch runtime.GOOS {
	case "linux":
		cmd := exec.Command("clear") //Linux example, its tested
		cmd.Stdout = os.Stdout
		cmd.Run()
	case "windows":
		cmd := exec.Command("cmd.exe", "/c", "cls") //Windows example it is untested, but I think its working
		cmd.Stdout = os.Stdout
		cmd.Run()
	}
}

func getRune(t string) string {
	switch t {
	case "grass":
		return "."
	case "water":
		return "~"
	case "mountain":
		return "M"
	default:
		return "?"
	}

}
func showMap(gamemap model.WorldMap) {
	//fmt.Println("showing map")
	b := make([][]string, gamemap.MapObj.Dimensions.Width)
	defaultTile := gamemap.MapObj.DefaultTile.TileType
	marker := getRune(defaultTile)

	for i := range b {
		b[i] = make([]string, gamemap.MapObj.Dimensions.Height)
		for v := range b[i] {
			b[i][v] = marker
		}
	}

	for _, tile := range gamemap.MapObj.Tiles {
		for _, tilelocation := range tile.TFeatures.Coordinates {
			b[tilelocation.Y][tilelocation.X] = getRune(tile.TileType)
		}
	}

	for _, something := range gamemap.Entities {
		b[something.Coordinates.Y][something.Coordinates.X] = "@"
	}

	for i := range b {
		fmt.Println(b[i])
	}

}

func GetMessages(ex chan []byte, c *websocket.Conn) {
	for {
		_, jsonData, err := c.ReadMessage()

		// umm wat? bye Felica
		if err != nil {
			fmt.Println(err)
		}

		ex <- jsonData
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

	// Silly message to test server message
	r := &model.ClientMessage{
		ModelType: model.ModelType{MsgType: model.CLIENTMESSAGE},
		Msg:       "move and groove"}
	wsConn.WriteJSON(r)

	// What's the world like?
	worldmap := &model.WorldMap{}

	// Who am I?
	whoiam := &model.PlayerInfo{}

	// Things I control
	myentities := []model.Entity{}

	frameNS := time.Duration(time.Second)
	clk := time.NewTicker(frameNS)

	// Receive from our websocket
	jsonExchange := make(chan []byte)
	go GetMessages(jsonExchange, wsConn)

	for {
		// receive from time based or websocket
		select {
		case <-clk.C:
			for _, entity := range myentities {
				// fmt.Println(num, entity)
				// a is for action
				a := &model.EntityAction{
					ModelType: model.ModelType{MsgType: model.ENTITYACTION},
					Action:    model.ENTITYACTIONMOVE,
					Entity:    model.Entity{ID: entity.ID},
					EntityMove: model.EntityMove{
						Direction: model.ENTITYDIRECTIONDOWN,
						Distance:  1},
				}

				wsConn.WriteJSON(a)
			}

		case jsonData := <-jsonExchange:

			// All messages start like this ;)
			cmsg := &model.ModelType{}

			err := json.Unmarshal(jsonData, cmsg)
			if err != nil {
				fmt.Println("GAMEOVER: ", err)
				break
			}

			// fmt.Println("MESSAGE TYPE:", cmsg.MsgType)

			// Determine message type
			switch cmsg.MsgType {

			// Receive client messages
			case model.CLIENTMESSAGE:
				m := &model.ClientMessage{}
				json.Unmarshal(jsonData, m)
				// fmt.Println("Recieved:", m.Msg, " From: ", m.Sender)

			// receive bootstap of map
			case model.WORLDMAP:

				json.Unmarshal(jsonData, worldmap)
				showMap(*worldmap)

				for _, entity := range worldmap.Entities {
					if entity.Owner == whoiam.ID {
						myentities = append(myentities, entity)
					}
				}

			case model.PLAYERINFO:
				json.Unmarshal(jsonData, whoiam)
				// fmt.Println("WHO I AM:", whoiam)

			case model.ENTITY:
				ent := &model.Entity{}
				json.Unmarshal(jsonData, ent)
				// fmt.Println("ENT!", ent)
				for index, entity := range worldmap.Entities {
					if entity.ID == ent.ID {
						worldmap.Entities[index] = *ent
					}
				}
				ClearScreen()
				showMap(*worldmap)

			default:
				fmt.Println("UNKNOWN MESSAGE:", cmsg.MsgType)
			}

		}
	}
}
